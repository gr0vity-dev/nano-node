use std::sync::Arc;

use anyhow::Result;
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::{Amount, PublicKey};
use store_traits::{
    environment::StoreCursor,
    ledger::{RepWeightStore, StoreIterator},
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::{StoreDatabase, StoreWriteFlags},
};

use crate::{
    REP_WEIGHT_CF_NAME, RocksdbCursor, RocksdbStoreEnvironment, rocksdb_ro_cursor_from_store,
};

pub struct RocksdbRepWeightStore {
    database: StoreDatabase,
    delete_listener: OutputListenerMt<PublicKey>,
    put_listener: OutputListenerMt<(PublicKey, Amount)>,
}

impl RocksdbRepWeightStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let database = env.open_db(Some(REP_WEIGHT_CF_NAME))?;
        Ok(Self {
            database,
            delete_listener: OutputListenerMt::new(),
            put_listener: OutputListenerMt::new(),
        })
    }

    fn database(&self) -> StoreDatabase {
        self.database
    }

    pub fn track_deletions(&self) -> Arc<OutputTrackerMt<PublicKey>> {
        self.delete_listener.track()
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<(PublicKey, Amount)>> {
        self.put_listener.track()
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn, pub_key: &PublicKey) -> Option<Amount> {
        match txn.get(self.database(), pub_key.as_bytes()) {
            Ok(mut bytes) => Amount::deserialize(&mut bytes).ok(),
            Err(e) if e.is_not_found() => None,
            // TODO(store-errors): propagate backend errors instead of panicking once traits return StoreResult.
            Err(e) => panic!("failed to read rep weight: {e}"),
        }
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, representative: PublicKey, weight: Amount) {
        if self.put_listener.is_tracked() {
            self.put_listener.emit((representative, weight));
        }
        txn.put(
            self.database(),
            representative.as_bytes(),
            &weight.to_be_bytes(),
            StoreWriteFlags::default(),
        )
        .expect("failed to write rep weight");
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, representative: &PublicKey) {
        if self.delete_listener.is_tracked() {
            self.delete_listener.emit(*representative);
        }
        txn.delete(self.database(), representative.as_bytes(), None)
            .expect("failed to delete rep weight");
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.raw_count(self.database())
    }

    pub fn iter<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
    ) -> StoreIterator<'txn, (PublicKey, Amount)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open rep weight cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbRepWeightIterator::new(cursor))
    }
}

impl RepWeightStore for RocksdbRepWeightStore {
    fn get(&self, txn: &dyn LedgerReadTxn, rep: &PublicKey) -> Option<Amount> {
        RocksdbRepWeightStore::get(self, txn, rep)
    }

    fn put(&self, txn: &mut dyn LedgerWriteTxn, representative: PublicKey, weight: Amount) {
        RocksdbRepWeightStore::put(self, txn, representative, weight);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, representative: &PublicKey) {
        RocksdbRepWeightStore::del(self, txn, representative);
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(PublicKey, Amount)>> {
        RocksdbRepWeightStore::track_puts(self)
    }

    fn track_deletions(&self) -> Arc<OutputTrackerMt<PublicKey>> {
        RocksdbRepWeightStore::track_deletions(self)
    }
}

struct RocksdbRepWeightIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
}

impl<'txn> RocksdbRepWeightIterator<'txn> {
    fn new(cursor: RocksdbCursor<'txn>) -> Self {
        Self { cursor }
    }
}

impl<'txn> Iterator for RocksdbRepWeightIterator<'txn> {
    type Item = (PublicKey, Amount);

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.cursor.next().expect("failed to advance cursor")?;
        Some(read_rep_weight_record(entry))
    }
}

fn read_rep_weight_record((key, value): (&[u8], &[u8])) -> (PublicKey, Amount) {
    let pub_key = PublicKey::from_slice(
        key.try_into()
            .expect("invalid representative key length in RocksDB"),
    )
    .expect("failed to parse public key");
    let mut bytes = value;
    let amount = Amount::deserialize(&mut bytes).expect("failed to deserialize amount");
    (pub_key, amount)
}
