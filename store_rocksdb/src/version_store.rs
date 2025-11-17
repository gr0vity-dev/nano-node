use std::sync::Arc;

use anyhow::Result;
use store_traits::{
    ledger::VersionStore,
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::{StoreDatabase, StoreWriteFlags},
};

use crate::{RocksdbStoreEnvironment, VERSION_CF_NAME};

pub struct RocksdbVersionStore {
    database: StoreDatabase,
}

impl RocksdbVersionStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let database = env.open_db(Some(VERSION_CF_NAME))?;
        Ok(Self { database })
    }

    fn database(&self) -> StoreDatabase {
        self.database
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, version: i32) {
        let key = version_key();
        let value = version_value(version);
        txn.put(self.database(), &key, &value, StoreWriteFlags::default())
            .expect("failed to write version");
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn) -> Option<i32> {
        let key = version_key();
        match txn.get(self.database(), &key) {
            Ok(value) => Some(decode_version(value)),
            Err(e) if e.is_not_found() => None,
            // TODO(store-errors): propagate backend errors instead of panicking once traits return StoreResult.
            Err(e) => panic!("failed to read version: {e}"),
        }
    }
}

fn version_value(version: i32) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[28..].copy_from_slice(&version.to_be_bytes());
    bytes
}

fn version_key() -> [u8; 32] {
    version_value(1)
}

fn decode_version(bytes: &[u8]) -> i32 {
    let mut array = [0u8; 4];
    array.copy_from_slice(&bytes[28..32]);
    i32::from_be_bytes(array)
}

impl VersionStore for RocksdbVersionStore {
    fn get(&self, txn: &dyn LedgerReadTxn) -> Option<i32> {
        RocksdbVersionStore::get(self, txn)
    }
}
