use std::sync::Arc;

use anyhow::Result;
use rsnano_types::Amount;
use store_traits::{
    environment::StoreCursor,
    ledger::{OnlineWeightStore, StoreIterator},
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::{StoreDatabase, StoreWriteFlags},
};

use crate::{
    ONLINE_WEIGHT_CF_NAME, RocksdbCursor, RocksdbStoreEnvironment, rocksdb_ro_cursor_from_store,
};

pub struct RocksdbOnlineWeightStore {
    database: StoreDatabase,
}

impl RocksdbOnlineWeightStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let database = env.open_db(Some(ONLINE_WEIGHT_CF_NAME))?;
        Ok(Self { database })
    }

    fn database(&self) -> StoreDatabase {
        self.database
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, time: u64, amount: &Amount) {
        txn.put(
            self.database(),
            &time.to_be_bytes(),
            &amount.to_be_bytes(),
            StoreWriteFlags::default(),
        )
        .expect("failed to write online weight");
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, time: u64) {
        txn.delete(self.database(), &time.to_be_bytes(), None)
            .expect("failed to delete online weight");
    }

    pub fn iter<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
    ) -> StoreIterator<'txn, (u64, Amount)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open online weight cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbOnlineWeightIterator::new(cursor))
    }

    pub fn iter_rev<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
    ) -> StoreIterator<'txn, (u64, Amount)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open online weight cursor");
        let mut cursor = rocksdb_ro_cursor_from_store(cursor);
        let mut entries = Vec::new();
        loop {
            match cursor.next().expect("failed to advance RocksDB cursor") {
                Some((key, value)) => {
                    let time = u64::from_be_bytes(
                        key.try_into().expect("invalid online weight key length"),
                    );
                    let amount = Amount::from_be_bytes(
                        value
                            .try_into()
                            .expect("invalid online weight amount length"),
                    );
                    entries.push((time, amount));
                }
                None => break,
            }
        }
        entries.reverse();
        Box::new(entries.into_iter())
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.raw_count(self.database())
    }

    pub fn clear(&self, txn: &mut dyn LedgerWriteTxn) {
        txn.clear_db(self.database())
            .expect("failed to clear online weight");
    }
}

impl OnlineWeightStore for RocksdbOnlineWeightStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, time: u64, amount: &Amount) {
        RocksdbOnlineWeightStore::put(self, txn, time, amount);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, time: u64) {
        RocksdbOnlineWeightStore::del(self, txn, time);
    }

    fn iter<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, (u64, Amount)> {
        RocksdbOnlineWeightStore::iter(self, txn)
    }

    fn iter_rev<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, (u64, Amount)> {
        RocksdbOnlineWeightStore::iter_rev(self, txn)
    }
}

struct RocksdbOnlineWeightIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
}

impl<'txn> RocksdbOnlineWeightIterator<'txn> {
    fn new(cursor: RocksdbCursor<'txn>) -> Self {
        Self { cursor }
    }
}

impl<'txn> Iterator for RocksdbOnlineWeightIterator<'txn> {
    type Item = (u64, Amount);

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.cursor.next().expect("failed to advance cursor")?;
        let time = u64::from_be_bytes(entry.0.try_into().expect("invalid time bytes"));
        let amount = Amount::from_be_bytes(entry.1.try_into().expect("invalid amount bytes"));
        Some((time, amount))
    }
}
