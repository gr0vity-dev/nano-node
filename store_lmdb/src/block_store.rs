use std::{
    ops::RangeBounds,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use rsnano_nullable_lmdb::{
    ConfiguredDatabase, DatabaseFlags, Error, LmdbDatabase, LmdbEnvironment, Transaction,
    WriteFlags, sys::MDB_LAST,
};
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::{BlockHash, SavedBlock};
use store_traits::transaction::{LedgerReadTxn, LedgerWriteTxn};

use crate::{BLOCK_DATA_DATABASE, BLOCK_INDEX_DATABASE, LmdbIterator, LmdbRangeIterator};

pub struct LmdbBlockStore {
    /// block hash => id
    index_db: LmdbDatabase,

    /// id => block data
    block_db: LmdbDatabase,

    put_listener: OutputListenerMt<SavedBlock>,
    next_id: AtomicU64,
}

pub struct ConfiguredBlockDatabaseBuilder {
    index_db: ConfiguredDatabase,
    block_db: ConfiguredDatabase,
    next_id: u64,
}

impl ConfiguredBlockDatabaseBuilder {
    pub fn new() -> Self {
        Self {
            index_db: ConfiguredDatabase::new(BLOCK_INDEX_DATABASE, BLOCK_INDEX_DB_NAME),
            block_db: ConfiguredDatabase::new(BLOCK_DATA_DATABASE, BLOCK_DATA_DB_NAME),
            next_id: 0,
        }
    }

    pub fn block(mut self, block: &SavedBlock) -> Self {
        let id = self.next_id;
        self.next_id += 1;

        self.index_db
            .insert(block.hash().as_bytes(), &id.to_be_bytes());
        self.block_db
            .insert(&id.to_be_bytes(), block.serialize_with_sideband());
        self
    }

    pub fn build(self) -> (ConfiguredDatabase, ConfiguredDatabase) {
        (self.index_db, self.block_db)
    }
}

impl LmdbBlockStore {
    pub fn configured_responses() -> ConfiguredBlockDatabaseBuilder {
        ConfiguredBlockDatabaseBuilder::new()
    }

    pub fn new(env: &LmdbEnvironment) -> anyhow::Result<Self> {
        let index_db = env.create_db(Some(BLOCK_INDEX_DB_NAME), DatabaseFlags::empty())?;
        let block_db = env.create_db(Some(BLOCK_DATA_DB_NAME), DatabaseFlags::empty())?;

        let next_id = find_next_free_id(env, block_db)?;

        Ok(Self {
            index_db,
            block_db,
            put_listener: OutputListenerMt::new(),
            next_id: AtomicU64::new(next_id),
        })
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<SavedBlock>> {
        self.put_listener.track()
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, block: &SavedBlock) {
        if self.put_listener.is_tracked() {
            self.put_listener.emit(block.clone());
        }

        self.raw_put(txn, &block.serialize_with_sideband(), &block.hash());
    }

    pub fn exists(&self, transaction: &dyn LedgerReadTxn, hash: &BlockHash) -> bool {
        transaction.raw_exists(self.index_db, hash.as_bytes())
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn, hash: &BlockHash) -> Option<SavedBlock> {
        self.block_raw_get(txn, hash).map(|mut block_bytes| {
            SavedBlock::deserialize(&mut block_bytes)
                .unwrap_or_else(|e| panic!("Could not deserialize block {}: {:?}", hash, e))
        })
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, hash: &BlockHash) {
        let id = match txn.raw_get(self.index_db, hash.as_bytes()) {
            Ok(id_bytes) => get_block_id(id_bytes),
            Err(Error::NotFound) => return,
            Err(e) => panic!("Could not delete block: {e:?} (hash: {hash})"),
        };

        txn.raw_delete(self.block_db, &id.to_be_bytes(), None)
            .expect("Could not delete block data (ID: {id})");
        txn.raw_delete(self.index_db, hash.as_bytes(), None)
            .expect("Could not delete block index (hash: {hash})");
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.raw_count(self.index_db)
    }

    pub fn iter<'tx>(
        &'tx self,
        tx: &'tx dyn LedgerReadTxn,
    ) -> impl Iterator<Item = SavedBlock> + 'tx {
        let cursor = tx
            .raw_open_ro_cursor(self.index_db)
            .expect("Could not open cursor for block index table");

        LmdbIterator::new(cursor, read_block_index_record).map(move |(_, id)| {
            let mut data = tx
                .raw_get(self.block_db, &id.to_be_bytes())
                .expect("Block data should exist");

            SavedBlock::deserialize(&mut data).expect("Block data should be valid")
        })
    }

    pub fn iter_range<'txn, R>(
        &'txn self,
        tx: &'txn dyn LedgerReadTxn,
        range: R,
    ) -> impl Iterator<Item = SavedBlock> + 'txn
    where
        R: RangeBounds<BlockHash> + 'static,
    {
        let cursor = tx
            .raw_open_ro_cursor(self.index_db)
            .expect("Could not open cursor for block table");

        LmdbRangeIterator::<BlockHash, u64>::new(
            cursor,
            range.start_bound().map(|b| b.as_bytes().to_vec()),
            range.end_bound().map(|b| b.as_bytes().to_vec()),
            read_block_index_record,
        )
        .map(move |(_, id)| {
            let mut data = tx
                .raw_get(self.block_db, &id.to_be_bytes())
                .expect("Block data should exist");

            let block = SavedBlock::deserialize(&mut data).expect("Block data should be valid");
            block
        })
    }

    fn raw_put(&self, txn: &mut dyn LedgerWriteTxn, data: &[u8], hash: &BlockHash) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);

        txn.raw_put(
            self.index_db,
            hash.as_bytes(),
            &id.to_be_bytes(),
            WriteFlags::NO_OVERWRITE,
        )
        .expect("Couldn't insert into block index table");

        txn.raw_put(self.block_db, &id.to_be_bytes(), data, WriteFlags::APPEND)
            .expect("Couldn't insert into block data table'");
    }

    fn block_raw_get<'a>(&self, txn: &'a dyn LedgerReadTxn, hash: &BlockHash) -> Option<&'a [u8]> {
        match txn.raw_get(self.index_db, hash.as_bytes()) {
            Err(Error::NotFound) => None,
            Ok(id_bytes) => Some(
                txn.raw_get(self.block_db, id_bytes)
                    .expect("Block data missing"),
            ),
            Err(e) => panic!("Could not load block. {:?}", e),
        }
    }
}

fn find_next_free_id(env: &LmdbEnvironment, block_db: LmdbDatabase) -> Result<u64, anyhow::Error> {
    let tx = env.begin_read();
    let cursor = tx.open_ro_cursor(block_db)?;
    match cursor.get(None, None, MDB_LAST) {
        Ok((Some(key), _)) => Ok(get_block_id(key) + 1),
        Ok((None, _)) => panic!("No key!"),
        Err(Error::NotFound) => Ok(0),
        Err(e) => Err(anyhow!("Couldn't load highest block id: {e:?}")),
    }
}

pub(crate) const BLOCK_INDEX_DB_NAME: &str = "block_index";
pub(crate) const BLOCK_DATA_DB_NAME: &str = "block_data";

fn read_block_index_record(k: &[u8], v: &[u8]) -> (BlockHash, u64) {
    (
        BlockHash::from_slice(k).expect("Should be valid block hash bytes"),
        get_block_id(v),
    )
}

fn get_block_id(id_bytes: &[u8]) -> u64 {
    u64::from_be_bytes(id_bytes.try_into().expect("Invalid block ID"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_nullable_lmdb::PutEvent;

    struct Fixture {
        env: Arc<LmdbEnvironment>,
        store: LmdbBlockStore,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_env(LmdbEnvironment::new_null())
        }

        fn with_env(env: LmdbEnvironment) -> Self {
            let env = Arc::new(env);
            Self {
                env: env.clone(),
                store: LmdbBlockStore::new(&env).unwrap(),
            }
        }
    }

    #[test]
    fn empty() {
        let fixture = Fixture::new();
        let store = &fixture.store;
        let txn = fixture.env.begin_read();

        assert!(store.get(&txn, &BlockHash::from(1)).is_none());
        assert_eq!(store.exists(&txn, &BlockHash::from(1)), false);
        assert_eq!(store.count(&txn), 0);
    }

    #[test]
    fn load_block_by_hash() {
        let block = SavedBlock::new_test_instance();

        let env = LmdbEnvironment::null_builder()
            .database(BLOCK_INDEX_DB_NAME, LmdbDatabase::new_null(99))
            .entry(block.hash().as_bytes(), &1u64.to_be_bytes())
            .build()
            .database(BLOCK_DATA_DB_NAME, LmdbDatabase::new_null(100))
            .entry(&1u64.to_be_bytes(), &block.serialize_with_sideband())
            .build()
            .build();

        let fixture = Fixture::with_env(env);
        let txn = fixture.env.begin_read();

        let result = fixture.store.get(&txn, &block.hash());
        assert_eq!(result, Some(block));
    }

    #[test]
    fn add_block() {
        let fixture = Fixture::new();
        let mut txn = fixture.env.begin_write();
        let put_tracker = txn.track_puts();
        let block = SavedBlock::new_test_open_block();

        fixture.store.put(&mut txn, &block);

        assert_eq!(
            put_tracker.output(),
            vec![
                PutEvent {
                    database: LmdbDatabase::new_null(42),
                    key: block.hash().as_bytes().to_vec(),
                    value: 0u64.to_be_bytes().to_vec(),
                    flags: WriteFlags::NO_OVERWRITE,
                },
                PutEvent {
                    database: LmdbDatabase::new_null(43),
                    key: 0u64.to_be_bytes().to_vec(),
                    value: block.serialize_with_sideband(),
                    flags: WriteFlags::APPEND,
                }
            ]
        );
    }

    #[test]
    fn track_inserted_blocks() {
        let fixture = Fixture::new();
        let block = SavedBlock::new_test_open_block();
        let mut txn = fixture.env.begin_write();
        let put_tracker = fixture.store.track_puts();

        fixture.store.put(&mut txn, &block);

        assert_eq!(put_tracker.output(), vec![block]);
    }

    #[test]
    fn can_be_nulled() {
        let block = SavedBlock::new_test_instance();
        let (block_index, block_data) =
            LmdbBlockStore::configured_responses().block(&block).build();

        let env = LmdbEnvironment::null_builder()
            .configured_database(block_index)
            .configured_database(block_data)
            .build();
        let txn = env.begin_read();
        let block_store = LmdbBlockStore::new(&env).unwrap();
        assert_eq!(block_store.get(&txn, &block.hash()), Some(block));
    }
}
