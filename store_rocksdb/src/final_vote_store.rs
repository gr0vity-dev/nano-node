use std::sync::Arc;

use anyhow::Result;
use rsnano_types::{BlockHash, QualifiedRoot};
use store_traits::{
    ledger::FinalVoteStore,
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::{StoreDatabase, StoreWriteFlags},
};

use crate::{FINAL_VOTE_CF_NAME, RocksdbStoreEnvironment};

pub struct RocksdbFinalVoteStore {
    database: StoreDatabase,
}

impl RocksdbFinalVoteStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let database = env.open_db(Some(FINAL_VOTE_CF_NAME))?;
        Ok(Self { database })
    }

    fn database(&self) -> StoreDatabase {
        self.database
    }

    pub fn put(
        &self,
        txn: &mut dyn LedgerWriteTxn,
        root: &QualifiedRoot,
        hash: &BlockHash,
    ) -> bool {
        let key = root.to_bytes();
        match txn.get(self.database(), &key) {
            Err(e) if e.is_not_found() => {
                txn.put(
                    self.database(),
                    &key,
                    hash.as_bytes(),
                    StoreWriteFlags::default(),
                )
                .expect("failed to insert final vote");
                true
            }
            Ok(existing) => {
                let stored = BlockHash::from_slice(existing.as_ref())
                    .expect("invalid block hash stored in final vote");
                stored == *hash
            }
            // TODO(store-errors): propagate backend errors instead of panicking once traits return StoreResult.
            Err(e) => panic!("failed to read final vote: {e}"),
        }
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn, root: &QualifiedRoot) -> Option<BlockHash> {
        match txn.get(self.database(), &root.to_bytes()) {
            Ok(bytes) => {
                let mut slice = bytes.as_ref();
                Some(BlockHash::deserialize(&mut slice).expect("failed to deserialize block hash"))
            }
            Err(e) if e.is_not_found() => None,
            // TODO(store-errors): propagate backend errors instead of panicking once traits return StoreResult.
            Err(e) => panic!("failed to read final vote: {e}"),
        }
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, root: &QualifiedRoot) {
        let key = root.to_bytes();
        txn.delete(self.database(), &key, None)
            .expect("failed to delete final vote");
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.raw_count(self.database())
    }

    pub fn clear(&self, txn: &mut dyn LedgerWriteTxn) {
        txn.clear_db(self.database())
            .expect("failed to clear final votes");
    }
}

impl FinalVoteStore for RocksdbFinalVoteStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, root: &QualifiedRoot, hash: &BlockHash) -> bool {
        RocksdbFinalVoteStore::put(self, txn, root, hash)
    }

    fn get(&self, txn: &dyn LedgerReadTxn, root: &QualifiedRoot) -> Option<BlockHash> {
        RocksdbFinalVoteStore::get(self, txn, root)
    }
}
