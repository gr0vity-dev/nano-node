use std::{ops::Bound, sync::Arc};

use anyhow::Result;
use rsnano_types::{Account, ConfirmationHeightInfo};
use store_traits::{
    environment::StoreCursor,
    ledger::{ConfirmationHeightStore, RangeBounds, StoreIterator},
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::{StoreDatabase, StoreValue, StoreWriteFlags},
};

use crate::{
    CONF_HEIGHT_CF_NAME, RocksdbCursor, RocksdbStoreEnvironment, rocksdb_ro_cursor_from_store,
    value_in_range,
};

pub struct RocksdbConfirmationHeightStore {
    database: StoreDatabase,
}

impl RocksdbConfirmationHeightStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let database = env.open_db(Some(CONF_HEIGHT_CF_NAME))?;
        Ok(Self { database })
    }

    fn database(&self) -> StoreDatabase {
        self.database
    }

    pub fn put(
        &self,
        txn: &mut dyn LedgerWriteTxn,
        account: &Account,
        info: &ConfirmationHeightInfo,
    ) {
        txn.put(
            self.database(),
            account.as_bytes(),
            &info.to_bytes(),
            StoreWriteFlags::default(),
        )
        .expect("failed to write confirmation height info");
    }

    pub fn get(
        &self,
        txn: &dyn LedgerReadTxn,
        account: &Account,
    ) -> Option<ConfirmationHeightInfo> {
        match txn.get(self.database(), account.as_bytes()) {
            Ok(bytes) => {
                let mut slice = bytes.as_ref();
                ConfirmationHeightInfo::deserialize(&mut slice).ok()
            }
            Err(e) if e.is_not_found() => None,
            // TODO(store-errors): propagate backend errors instead of panicking once traits return StoreResult.
            Err(e) => panic!("failed to read confirmation height: {e}"),
        }
    }

    pub fn exists(&self, txn: &dyn LedgerReadTxn, account: &Account) -> bool {
        txn.raw_exists(self.database(), account.as_bytes())
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, account: &Account) {
        txn.delete(self.database(), account.as_bytes(), None)
            .expect("failed to delete confirmation height");
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.raw_count(self.database())
    }

    pub fn clear(&self, txn: &mut dyn LedgerWriteTxn) {
        txn.clear_db(self.database())
            .expect("failed to clear confirmation height");
    }

    pub fn iter<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
    ) -> StoreIterator<'txn, (Account, ConfirmationHeightInfo)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open confirmation height cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbConfirmationHeightIterator::new(cursor))
    }

    pub fn iter_range<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
        range: RangeBounds<Account>,
    ) -> StoreIterator<'txn, (Account, ConfirmationHeightInfo)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open confirmation height cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbConfirmationHeightRangeIterator::new(cursor, range))
    }
}

impl ConfirmationHeightStore for RocksdbConfirmationHeightStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, account: &Account, info: &ConfirmationHeightInfo) {
        RocksdbConfirmationHeightStore::put(self, txn, account, info);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, account: &Account) -> Option<ConfirmationHeightInfo> {
        RocksdbConfirmationHeightStore::get(self, txn, account)
    }

    fn exists(&self, txn: &dyn LedgerReadTxn, account: &Account) -> bool {
        RocksdbConfirmationHeightStore::exists(self, txn, account)
    }

    fn iter<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
    ) -> StoreIterator<'a, (Account, ConfirmationHeightInfo)> {
        RocksdbConfirmationHeightStore::iter(self, txn)
    }
}

struct RocksdbConfirmationHeightIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
}

impl<'txn> RocksdbConfirmationHeightIterator<'txn> {
    fn new(cursor: RocksdbCursor<'txn>) -> Self {
        Self { cursor }
    }
}

impl<'txn> Iterator for RocksdbConfirmationHeightIterator<'txn> {
    type Item = (Account, ConfirmationHeightInfo);

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.cursor.next().expect("failed to advance cursor")?;
        Some(read_confirmation_height_record(entry))
    }
}

struct RocksdbConfirmationHeightRangeIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
    range: RangeBounds<Account>,
    initialized: bool,
}

impl<'txn> RocksdbConfirmationHeightRangeIterator<'txn> {
    fn new(cursor: RocksdbCursor<'txn>, range: RangeBounds<Account>) -> Self {
        Self {
            cursor,
            range,
            initialized: false,
        }
    }

    fn seek_start(&mut self) -> store_traits::types::StoreResult<Option<(StoreValue, StoreValue)>> {
        match &self.range.start {
            Bound::Included(account) => self.cursor.seek_lower_bound(account.as_bytes()),
            Bound::Excluded(account) => self.cursor.seek_upper_bound(account.as_bytes()),
            Bound::Unbounded => self.cursor.next(),
        }
    }

    fn advance_cursor(
        &mut self,
    ) -> store_traits::types::StoreResult<Option<(StoreValue, StoreValue)>> {
        if self.initialized {
            self.cursor.next()
        } else {
            self.initialized = true;
            self.seek_start()
        }
    }
}

impl<'txn> Iterator for RocksdbConfirmationHeightRangeIterator<'txn> {
    type Item = (Account, ConfirmationHeightInfo);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let entry = self
                .advance_cursor()
                .expect("failed to advance confirmation height cursor")?;
            let record = read_confirmation_height_record(entry);
            if value_in_range(&record.0, &self.range) {
                return Some(record);
            } else {
                return None;
            }
        }
    }
}

fn read_confirmation_height_record(
    (key, value): (StoreValue, StoreValue),
) -> (Account, ConfirmationHeightInfo) {
    let account = Account::from_bytes(
        key.as_ref()
            .try_into()
            .expect("invalid confirmation height key length"),
    );
    let mut slice = value.as_ref();
    let info = ConfirmationHeightInfo::deserialize(&mut slice)
        .expect("failed to deserialize confirmation height");
    (account, info)
}
