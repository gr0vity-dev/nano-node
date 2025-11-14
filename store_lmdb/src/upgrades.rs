use num_traits::FromPrimitive;
use tracing::{debug, error, info};

use rsnano_nullable_lmdb::{
    DatabaseFlags, EnvironmentOptions, LmdbEnvironment, LmdbEnvironmentFactory, Transaction,
    WriteFlags,
    sys::{MDB_FIRST, MDB_NEXT},
};
use rsnano_types::{BlockType, BlockTypeId, UnixMillisTimestamp, UnixTimestamp};

use crate::{
    FIRST_INCOMPATIBLE_STORE_VERSION, LmdbVersionStore, STORE_VERSION_CURRENT,
    STORE_VERSION_MINIMUM,
    block_store::{BLOCK_DATA_DB_NAME, BLOCK_INDEX_DB_NAME},
    transaction::LmdbLedgerWriteTxn,
    vacuum::vacuum,
};

#[cfg(test)]
use crate::transaction::LmdbLedgerReadTxn;

pub fn create_and_update_lmdb_env(
    env_factory: &LmdbEnvironmentFactory,
    options: EnvironmentOptions,
) -> anyhow::Result<LmdbEnvironment> {
    let mut env = env_factory.create(options.clone())?;
    let needs_vacuuming = upgrade_if_needed(&mut env)?;
    if needs_vacuuming {
        vacuum(env)?;
        env = env_factory.create(options)?;
    }
    Ok(env)
}

fn upgrade_if_needed(env: &mut LmdbEnvironment) -> Result<bool, anyhow::Error> {
    let upgrade_info = LmdbVersionStore::check_upgrade(&env)?;
    if upgrade_info.is_fully_upgraded {
        debug!("No database upgrade needed");
        return Ok(false);
    }

    info!("Upgrade in progress...");
    let needs_vacuuming = do_upgrades(env)?;
    info!("Upgrade done!");
    env.sync()?;
    Ok(needs_vacuuming)
}

fn do_upgrades(env: &mut LmdbEnvironment) -> anyhow::Result<bool> {
    let version_store = LmdbVersionStore::new(env)?;

    let mut version = {
        let mut txn = LmdbLedgerWriteTxn::new(env.begin_write());
        let version = match version_store.get(&txn) {
            Some(v) => v,
            None => {
                let new_version = STORE_VERSION_CURRENT;
                info!("Setting db version to {}", new_version);
                version_store.put(&mut txn, new_version);
                new_version
            }
        };
        txn.into_inner().commit();
        version
    };

    if version == STORE_VERSION_CURRENT {
        return Ok(false);
    }

    if version < STORE_VERSION_MINIMUM {
        error!(
            "The version of the ledger ({version}) is lower than the minimum ({STORE_VERSION_MINIMUM}) which is supported for upgrades. Either upgrade to an older version of RsNano first or delete the ledger."
        );
        bail!("version too low");
    }

    if version > STORE_VERSION_MINIMUM && version < FIRST_INCOMPATIBLE_STORE_VERSION {
        error!("The version of the ledger ({version}) is not supported for upgrades!");
        bail!("unsupported version");
    }

    if version > STORE_VERSION_CURRENT {
        error!("The version of the ledger ({version}) is too high for this node");
        bail!("version too high");
    }

    let mut needs_vacuuming = false;

    loop {
        if version == STORE_VERSION_CURRENT {
            break;
        }

        match version {
            24 => {
                create_successor_table(env)?;
                needs_vacuuming = true;
            }
            10_000 => {
                remove_successor_from_sideband_and_upgrade_timestamp_and_split_table(env)?;
                needs_vacuuming = true;
            }
            _ => unreachable!(),
        };

        version = next_version(version);

        let mut txn = LmdbLedgerWriteTxn::new(env.begin_write());
        version_store.put(&mut txn, version);
        txn.into_inner().commit();
    }

    if needs_vacuuming {
        //env.vacuum()?;
    }

    Ok(needs_vacuuming)
}

fn next_version(version: i32) -> i32 {
    if version == 24 {
        10_000 // switch to RsNano store versions
    } else {
        version + 1
    }
}

fn create_successor_table(env: &LmdbEnvironment) -> Result<(), anyhow::Error> {
    info!("Creating block successor table...");

    let block_db = env.create_db(Some("blocks"), DatabaseFlags::empty())?;
    let successor_db = env.create_db(Some("successors"), DatabaseFlags::empty())?;

    let tx_read = env.begin_read();
    let mut tx_write = env.begin_write();
    let mut processed = 0;
    let mut cursor = tx_read.open_ro_cursor(block_db)?;

    for row in cursor.iter_start() {
        let (k, v) = row?;
        let successor = V24Sideband::new(v).successor();
        tx_write.put(successor_db, k, &successor, WriteFlags::APPEND)?;
        processed += 1;
        if processed % 500_000 == 0 {
            info!("Processed {processed} blocks");
        }
    }

    drop(cursor);
    tx_read.commit();
    tx_write.commit();

    Ok(())
}

fn remove_successor_from_sideband_and_upgrade_timestamp_and_split_table(
    env: &LmdbEnvironment,
) -> Result<(), anyhow::Error> {
    info!(
        "Removing successor from sideband and upgrading timestamp to milliseconds and splitting block table..."
    );

    let block_db = env.create_db(Some("blocks"), DatabaseFlags::empty())?;
    let index_db = env.create_db(Some(BLOCK_INDEX_DB_NAME), DatabaseFlags::empty())?;
    let block_data_db = env.create_db(Some(BLOCK_DATA_DB_NAME), DatabaseFlags::empty())?;

    let mut processed = 0;
    let tx_read = env.begin_read();
    let mut tx_write = env.begin_write();
    let cursor = tx_read.open_ro_cursor(block_db)?;
    let mut op = MDB_FIRST;
    let mut hash_bytes = [0; 32];
    let mut new_data = Vec::new();
    let mut next_id = 0_u64;

    loop {
        match cursor.get(None, None, op) {
            Ok((Some(k), v)) => {
                hash_bytes.copy_from_slice(k);

                let v24_sideband = V24Sideband::new(v);
                v24_sideband.remove_successor_and_upgrade_timestamp_to_millis(&mut new_data);

                let id = next_id;
                next_id += 1;
                tx_write.put(index_db, &hash_bytes, &id.to_be_bytes(), WriteFlags::APPEND)?;
                tx_write.put(
                    block_data_db,
                    &id.to_be_bytes(),
                    &new_data,
                    WriteFlags::APPEND,
                )?;
                processed += 1;
                op = MDB_NEXT;
                if processed % 500_000 == 0 {
                    info!("Processed {processed} blocks");
                }
            }
            Ok((None, _)) => bail!("Block data without key found!"),
            Err(rsnano_nullable_lmdb::Error::NotFound) => break,
            Err(e) => bail!("Could not iter blocks table: {e:?}"),
        }
    }
    drop(cursor);
    tx_read.commit();

    info!("Dropping old block table...");
    unsafe {
        tx_write.drop_db(block_db)?;
    }
    tx_write.commit();

    Ok(())
}

struct V24Sideband<'a> {
    data: &'a [u8],
}

impl<'a> V24Sideband<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    pub fn successor(&self) -> [u8; 32] {
        // the first value in the old sideband is the successor
        let successor_start = self.data.len() - self.sideband_len();

        self.data[successor_start..successor_start + 32]
            .try_into()
            .unwrap()
    }

    pub fn remove_successor_and_upgrade_timestamp_to_millis(&self, result: &mut Vec<u8>) {
        result.clear();
        result.extend_from_slice(self.block_without_sideband());
        result.extend_from_slice(&self.sideband_without_successor());
        self.upgrade_timestamp_to_millis(result);
    }

    fn sideband_without_successor(&self) -> &[u8] {
        let new_sideband_len = self.sideband_len() - 32;
        let new_sideband_start = self.data.len() - new_sideband_len;
        &self.data[new_sideband_start..]
    }

    fn block_without_sideband(&self) -> &[u8] {
        &self.data[..self.data.len() - self.sideband_len()]
    }

    fn sideband_len(&self) -> usize {
        v24_sideband_len(self.block_type())
    }

    fn block_type(&self) -> BlockType {
        BlockTypeId::from_u8(self.data[0])
            .expect("Should be valid block type id")
            .try_into()
            .expect("Should be valid block type")
    }

    fn upgrade_timestamp_to_millis(&self, result: &mut [u8]) {
        let timestamp_slice = self.timestamp_slice(result);
        let timestamp_seconds = UnixTimestamp::from_be_bytes(timestamp_slice.try_into().unwrap());
        let timestamp_millis = UnixMillisTimestamp::from(timestamp_seconds);
        timestamp_slice.copy_from_slice(&timestamp_millis.to_be_bytes());
    }

    fn timestamp_slice<'r>(&self, result: &'r mut [u8]) -> &'r mut [u8] {
        let mut start_index = result.len() - 8;
        if self.block_type() == BlockType::State {
            start_index -= 2; // details + source epoch bytes
        }
        &mut result[start_index..start_index + 8]
    }
}

fn v24_sideband_len(block_type: BlockType) -> usize {
    match block_type {
        BlockType::LegacySend => 48 + 32,
        BlockType::LegacyReceive => 64 + 32,
        BlockType::LegacyOpen => 24 + 32,
        BlockType::LegacyChange => 64 + 32,
        BlockType::State => 18 + 32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_types::{Block, BlockHash, BlockSideband};

    #[test]
    fn old_sideband_len() {
        let assert_len = |block_type: BlockType| {
            assert_eq!(
                v24_sideband_len(block_type),
                BlockSideband::serialized_size(block_type) + 32,
                "incorrect sideband len for {block_type:?}"
            );
        };
        assert_len(BlockType::LegacySend);
        assert_len(BlockType::LegacyReceive);
        assert_len(BlockType::LegacyOpen);
        assert_len(BlockType::LegacyChange);
        assert_len(BlockType::State);
    }

    #[test]
    fn get_successor_from_v24_sideband() {
        let mut buffer = Vec::new();
        let block = Block::new_test_instance();
        assert_eq!(block.block_type(), BlockType::State);
        block.serialize(&mut buffer).unwrap();
        let successor = BlockHash::from(12345);
        successor.serialize(&mut buffer).unwrap();
        buffer.extend_from_slice(&123_u64.to_be_bytes()); // block height
        buffer.extend_from_slice(&UnixTimestamp::from(123).to_be_bytes());
        buffer.push(42); // block details;
        buffer.push(42); // source epoch;

        let sideband = V24Sideband::new(&buffer);

        assert_eq!(sideband.successor(), *successor.as_bytes());
    }

    #[test]
    fn version_too_high_for_upgrade() -> anyhow::Result<()> {
        let env = LmdbEnvironment::new_null();
        set_store_version(&env, i32::MAX)?;
        assert_upgrade_fails(env, "version too high");
        Ok(())
    }

    #[test]
    fn version_too_low_for_upgrade() -> anyhow::Result<()> {
        let env = LmdbEnvironment::new_null();
        set_store_version(&env, STORE_VERSION_MINIMUM - 1)?;
        assert_upgrade_fails(env, "version too low");
        Ok(())
    }

    #[test]
    fn writes_db_version_for_new_store() {
        let mut env = LmdbEnvironment::new_null();
        upgrade_if_needed(&mut env).unwrap();
        let txn = LmdbLedgerReadTxn::new(env.begin_read());
        let version_store = LmdbVersionStore::new(&env).unwrap();
        assert_eq!(version_store.get(&txn), Some(STORE_VERSION_CURRENT));
    }

    fn assert_upgrade_fails(mut env: LmdbEnvironment, error_msg: &str) {
        let result = upgrade_if_needed(&mut env);
        match result {
            Ok(_) => panic!("store should not be created!"),
            Err(e) => {
                assert_eq!(e.to_string(), error_msg);
            }
        }
    }

    fn set_store_version(env: &LmdbEnvironment, current_version: i32) -> Result<(), anyhow::Error> {
        let version_store = LmdbVersionStore::new(env)?;
        let mut txn = LmdbLedgerWriteTxn::new(env.begin_write());
        version_store.put(&mut txn, current_version);
        txn.into_inner().commit();
        Ok(())
    }
}
