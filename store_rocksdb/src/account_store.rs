use std::{ops::Bound, sync::Arc};

use anyhow::Result;
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::{Account, AccountInfo};
use store_traits::{
    environment::StoreCursor,
    ledger::{AccountStore, RangeBounds, StoreIterator},
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::{StoreDatabase, StoreValue, StoreWriteFlags},
};

use crate::{
    ACCOUNTS_CF_NAME, RocksdbCursor, RocksdbStoreEnvironment, rocksdb_ro_cursor_from_store,
    value_in_range,
};

pub struct RocksdbAccountStore {
    database: StoreDatabase,
    put_listener: OutputListenerMt<(Account, AccountInfo)>,
}

impl RocksdbAccountStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let database = env.open_db(Some(ACCOUNTS_CF_NAME))?;
        Ok(Self {
            database,
            put_listener: OutputListenerMt::new(),
        })
    }

    fn database(&self) -> StoreDatabase {
        self.database
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<(Account, AccountInfo)>> {
        self.put_listener.track()
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, account: &Account, info: &AccountInfo) {
        if self.put_listener.is_tracked() {
            self.put_listener.emit((*account, info.clone()));
        }

        txn.put(
            self.database(),
            account.as_bytes(),
            &info.to_bytes(),
            StoreWriteFlags::default(),
        )
        .expect("failed to write account info");
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn, account: &Account) -> Option<AccountInfo> {
        match txn.get(self.database(), account.as_bytes()) {
            Ok(bytes) => {
                let mut slice = bytes.as_ref();
                AccountInfo::deserialize(&mut slice).ok()
            }
            Err(e) if e.is_not_found() => None,
            // TODO(store-errors): propagate backend errors instead of panicking once traits return StoreResult.
            Err(e) => panic!("failed to read account info: {e}"),
        }
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, account: &Account) {
        txn.delete(self.database(), account.as_bytes(), None)
            .expect("failed to delete account");
    }

    pub fn iter<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
    ) -> StoreIterator<'txn, (Account, AccountInfo)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open account cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbAccountIterator::new(cursor))
    }

    pub fn iter_range<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
        range: RangeBounds<Account>,
    ) -> StoreIterator<'txn, (Account, AccountInfo)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open account cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbAccountRangeIterator::new(cursor, range))
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.raw_count(self.database())
    }
}

impl AccountStore for RocksdbAccountStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, account: &Account, info: &AccountInfo) {
        RocksdbAccountStore::put(self, txn, account, info);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, account: &Account) -> Option<AccountInfo> {
        RocksdbAccountStore::get(self, txn, account)
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, account: &Account) {
        RocksdbAccountStore::del(self, txn, account);
    }

    fn iter<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, (Account, AccountInfo)> {
        RocksdbAccountStore::iter(self, txn)
    }

    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
        range: RangeBounds<Account>,
    ) -> StoreIterator<'a, (Account, AccountInfo)> {
        RocksdbAccountStore::iter_range(self, txn, range)
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(Account, AccountInfo)>> {
        RocksdbAccountStore::track_puts(self)
    }
}

struct RocksdbAccountIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
}

impl<'txn> RocksdbAccountIterator<'txn> {
    fn new(cursor: RocksdbCursor<'txn>) -> Self {
        Self { cursor }
    }
}

impl<'txn> Iterator for RocksdbAccountIterator<'txn> {
    type Item = (Account, AccountInfo);

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.cursor.next().expect("failed to advance cursor")?;
        Some(read_account_record(entry))
    }
}

struct RocksdbAccountRangeIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
    range: RangeBounds<Account>,
    initialized: bool,
}

impl<'txn> RocksdbAccountRangeIterator<'txn> {
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

impl<'txn> Iterator for RocksdbAccountRangeIterator<'txn> {
    type Item = (Account, AccountInfo);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let entry = self
                .advance_cursor()
                .expect("failed to advance account cursor")?;
            let record = read_account_record(entry);
            if value_in_range(&record.0, &self.range) {
                return Some(record);
            } else {
                return None;
            }
        }
    }
}

fn read_account_record((key, value): (StoreValue, StoreValue)) -> (Account, AccountInfo) {
    let account = Account::from_bytes(
        key.as_ref()
            .try_into()
            .expect("invalid account key length in RocksDB"),
    );
    let mut slice = value.as_ref();
    let info =
        AccountInfo::deserialize(&mut slice).expect("failed to deserialize RocksDB account info");
    (account, info)
}
