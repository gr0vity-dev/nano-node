use std::sync::Arc;

use anyhow::Result;
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::BlockHash;
use store_traits::{
    ledger::SuccessorStore,
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::{StoreDatabase, StoreWriteFlags},
};

use crate::{RocksdbStoreEnvironment, SUCCESSOR_CF_NAME};

pub struct RocksdbSuccessorStore {
    database: StoreDatabase,
    put_listener: OutputListenerMt<(BlockHash, BlockHash)>,
}

impl RocksdbSuccessorStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let database = env.open_db(Some(SUCCESSOR_CF_NAME))?;
        Ok(Self {
            database,
            put_listener: OutputListenerMt::new(),
        })
    }

    fn database(&self) -> StoreDatabase {
        self.database
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<(BlockHash, BlockHash)>> {
        self.put_listener.track()
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, block: &BlockHash, successor: &BlockHash) {
        if self.put_listener.is_tracked() {
            self.put_listener.emit((*block, *successor));
        }
        txn.put(
            self.database(),
            block.as_bytes(),
            successor.as_bytes(),
            StoreWriteFlags::default(),
        )
        .expect("failed to write successor");
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, block: &BlockHash) {
        txn.delete(self.database(), block.as_bytes(), None)
            .expect("failed to delete successor");
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn, block: &BlockHash) -> Option<BlockHash> {
        match txn.get(self.database(), block.as_bytes()) {
            Ok(bytes) => BlockHash::from_slice(bytes.as_ref()),
            Err(e) if e.is_not_found() => None,
            // TODO(store-errors): propagate backend errors instead of panicking once traits return StoreResult.
            Err(e) => panic!("failed to read successor: {e}"),
        }
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.raw_count(self.database())
    }
}

impl SuccessorStore for RocksdbSuccessorStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, block: &BlockHash, successor: &BlockHash) {
        RocksdbSuccessorStore::put(self, txn, block, successor);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, block: &BlockHash) {
        RocksdbSuccessorStore::del(self, txn, block);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, block: &BlockHash) -> Option<BlockHash> {
        RocksdbSuccessorStore::get(self, txn, block)
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(BlockHash, BlockHash)>> {
        RocksdbSuccessorStore::track_puts(self)
    }
}
