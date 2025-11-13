use rsnano_nullable_lmdb::{DatabaseFlags, LmdbEnvironment, WriteFlags};
use store_traits::transaction::{LedgerReadTxn, LedgerWriteTxn};

use crate::{LmdbDatabase, STORE_VERSION_CURRENT};

pub struct LmdbVersionStore {
    /// U256 (arbitrary key) -> blob
    db_handle: LmdbDatabase,
}

pub struct UpgradeInfo {
    pub is_fresh_db: bool,
    pub is_fully_upgraded: bool,
}

impl LmdbVersionStore {
    pub fn new(env: &LmdbEnvironment) -> anyhow::Result<Self> {
        let db_handle = env.create_db(Some("meta"), DatabaseFlags::empty())?;

        Ok(Self { db_handle })
    }

    pub fn try_read_version(env: &LmdbEnvironment) -> Option<i32> {
        match env.open_db(Some("meta")) {
            Ok(db) => {
                let txn = env.begin_read();
                load_version(&txn, db)
            }
            Err(_) => None,
        }
    }

    pub fn check_upgrade(env: &LmdbEnvironment) -> anyhow::Result<UpgradeInfo> {
        let info = match LmdbVersionStore::try_read_version(&env) {
            Some(version) => UpgradeInfo {
                is_fresh_db: false,
                is_fully_upgraded: version == STORE_VERSION_CURRENT,
            },
            None => UpgradeInfo {
                is_fresh_db: true,
                is_fully_upgraded: false,
            },
        };
        Ok(info)
    }

    pub fn db_handle(&self) -> LmdbDatabase {
        self.db_handle
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, version: i32) {
        let db = self.db_handle();

        let key_bytes = version_key();
        let value_bytes = value_bytes(version);

        txn.put(db.into(), &key_bytes, &value_bytes, WriteFlags::empty().into())
            .unwrap();
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn) -> Option<i32> {
        let db = self.db_handle();
        load_version(txn, db)
    }
}

fn load_version(txn: &dyn LedgerReadTxn, db: LmdbDatabase) -> Option<i32> {
    let key_bytes = version_key();
    match txn.get(db.into(), &key_bytes) {
        Ok(value) => Some(i32::from_be_bytes(value[28..].try_into().unwrap())),
        Err(e) if e.is_not_found() => None,
        Err(_) => panic!("Error while loading db version"),
    }
}

fn value_bytes(version: i32) -> [u8; 32] {
    let mut value_bytes = [0; 32];
    value_bytes[28..].copy_from_slice(&version.to_be_bytes());
    value_bytes
}

fn version_key() -> [u8; 32] {
    value_bytes(1)
}
