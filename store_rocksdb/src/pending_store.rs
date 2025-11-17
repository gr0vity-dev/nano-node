use std::{ops::Bound, sync::Arc};

use anyhow::Result;
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::{Account, BlockHash, PendingInfo, PendingKey};
use store_traits::{
    environment::StoreCursor,
    ledger::{PendingStore, RangeBounds, StoreIterator},
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::{StoreDatabase, StoreWriteFlags},
};

use crate::{
    PENDING_CF_NAME, RocksdbCursor, RocksdbStoreEnvironment, rocksdb_ro_cursor_from_store,
    value_in_range,
};

pub struct RocksdbPendingStore {
    database: StoreDatabase,
    put_listener: OutputListenerMt<(PendingKey, PendingInfo)>,
    delete_listener: OutputListenerMt<PendingKey>,
}

impl RocksdbPendingStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let database = env.open_db(Some(PENDING_CF_NAME))?;
        Ok(Self {
            database,
            put_listener: OutputListenerMt::new(),
            delete_listener: OutputListenerMt::new(),
        })
    }

    fn database(&self) -> StoreDatabase {
        self.database
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<(PendingKey, PendingInfo)>> {
        self.put_listener.track()
    }

    pub fn track_deletions(&self) -> Arc<OutputTrackerMt<PendingKey>> {
        self.delete_listener.track()
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey, info: &PendingInfo) {
        if self.put_listener.is_tracked() {
            self.put_listener.emit((key.clone(), info.clone()));
        }

        txn.put(
            self.database(),
            &key.to_bytes(),
            &info.to_bytes(),
            StoreWriteFlags::default(),
        )
        .expect("failed to write pending info");
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey) {
        if self.delete_listener.is_tracked() {
            self.delete_listener.emit(key.clone());
        }

        txn.delete(self.database(), &key.to_bytes(), None)
            .expect("failed to delete pending info");
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn, key: &PendingKey) -> Option<PendingInfo> {
        match txn.get(self.database(), &key.to_bytes()) {
            Ok(mut bytes) => Some(
                PendingInfo::deserialize(&mut bytes)
                    .expect("failed to deserialize RocksDB pending info"),
            ),
            Err(e) if e.is_not_found() => None,
            // TODO(store-errors): propagate backend errors instead of panicking once traits return StoreResult.
            Err(e) => panic!("failed to read pending info: {e}"),
        }
    }

    pub fn iter<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
    ) -> StoreIterator<'txn, (PendingKey, PendingInfo)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open pending cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbPendingIterator::new(cursor))
    }

    pub fn iter_range<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
        range: RangeBounds<PendingKey>,
    ) -> StoreIterator<'txn, (PendingKey, PendingInfo)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open pending cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbPendingRangeIterator::new(cursor, range))
    }

    pub fn exists(&self, txn: &dyn LedgerReadTxn, key: &PendingKey) -> bool {
        txn.raw_exists(self.database(), &key.to_bytes())
    }

    pub fn any(&self, txn: &dyn LedgerReadTxn, account: &Account) -> bool {
        let start = PendingKey::new(*account, BlockHash::ZERO);
        let range = RangeBounds::new(Bound::Included(start), Bound::Unbounded);
        self.iter_range(txn, range)
            .next()
            .map(|(key, _)| key.receiving_account == *account)
            .unwrap_or(false)
    }
}

impl PendingStore for RocksdbPendingStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey, pending: &PendingInfo) {
        RocksdbPendingStore::put(self, txn, key, pending);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey) {
        RocksdbPendingStore::del(self, txn, key);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, key: &PendingKey) -> Option<PendingInfo> {
        RocksdbPendingStore::get(self, txn, key)
    }

    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
        range: RangeBounds<PendingKey>,
    ) -> StoreIterator<'a, (PendingKey, PendingInfo)> {
        RocksdbPendingStore::iter_range(self, txn, range)
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(PendingKey, PendingInfo)>> {
        RocksdbPendingStore::track_puts(self)
    }

    fn track_deletions(&self) -> Arc<OutputTrackerMt<PendingKey>> {
        RocksdbPendingStore::track_deletions(self)
    }
}

struct RocksdbPendingIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
}

impl<'txn> RocksdbPendingIterator<'txn> {
    fn new(cursor: RocksdbCursor<'txn>) -> Self {
        Self { cursor }
    }
}

impl<'txn> Iterator for RocksdbPendingIterator<'txn> {
    type Item = (PendingKey, PendingInfo);

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.cursor.next().expect("failed to advance cursor")?;
        Some(read_pending_record(entry))
    }
}

struct RocksdbPendingRangeIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
    range: RangeBounds<PendingKey>,
}

impl<'txn> RocksdbPendingRangeIterator<'txn> {
    fn new(cursor: RocksdbCursor<'txn>, range: RangeBounds<PendingKey>) -> Self {
        Self { cursor, range }
    }
}

impl<'txn> Iterator for RocksdbPendingRangeIterator<'txn> {
    type Item = (PendingKey, PendingInfo);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let entry = self.cursor.next().expect("failed to advance cursor")?;
            let record = read_pending_record(entry);
            if value_in_range(&record.0, &self.range) {
                return Some(record);
            }
        }
    }
}

fn read_pending_record((key, value): (&[u8], &[u8])) -> (PendingKey, PendingInfo) {
    let mut key_bytes = key;
    let mut value_bytes = value;
    let key =
        PendingKey::deserialize(&mut key_bytes).expect("failed to deserialize RocksDB pending key");
    let info = PendingInfo::deserialize(&mut value_bytes)
        .expect("failed to deserialize RocksDB pending info");
    (key, info)
}
