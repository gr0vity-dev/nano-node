use std::sync::Arc;

use anyhow::Result;
use rsnano_types::BlockHash;
use store_traits::{
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::{StoreDatabase, StoreWriteFlags},
};

use crate::{PRUNED_CF_NAME, RocksdbStoreEnvironment};

pub struct RocksdbPrunedStore {
    database: StoreDatabase,
}

impl RocksdbPrunedStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let database = env.open_db(Some(PRUNED_CF_NAME))?;
        Ok(Self { database })
    }

    fn database(&self) -> StoreDatabase {
        self.database
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, hash: &BlockHash) {
        txn.put(
            self.database(),
            hash.as_bytes(),
            &[],
            StoreWriteFlags::default(),
        )
        .expect("failed to insert pruned hash");
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, hash: &BlockHash) {
        txn.delete(self.database(), hash.as_bytes(), None)
            .expect("failed to delete pruned hash");
    }

    pub fn exists(&self, txn: &dyn LedgerReadTxn, hash: &BlockHash) -> bool {
        txn.raw_exists(self.database(), hash.as_bytes())
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.raw_count(self.database())
    }
}
