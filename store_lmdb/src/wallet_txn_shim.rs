use std::time::Duration;

use rsnano_nullable_lmdb::{
    LmdbDatabase, ReadTransaction, RoCursor, RwCursor, Transaction as LmdbTransaction, WriteFlags,
    WriteTransaction,
};
use store_traits::transaction::{WalletReadTxn, WalletWriteTxn};

/// Temporary shim over an LMDB read transaction. Provides only the operations
/// wallet code needs while keeping LMDB types quarantined.
pub struct WalletReadTxnSHIM {
    inner: ReadTransaction,
}

impl WalletReadTxnSHIM {
    pub fn new(inner: ReadTransaction) -> Self {
        Self { inner }
    }

    pub fn commit(self) {
        self.inner.commit();
    }

    pub fn refresh(self) -> Self {
        Self {
            inner: self.inner.refresh(),
        }
    }

    pub fn get(&self, database: LmdbDatabase, key: &[u8]) -> rsnano_nullable_lmdb::Result<&[u8]> {
        self.inner.get(database, key)
    }

    pub fn open_ro_cursor(
        &self,
        database: LmdbDatabase,
    ) -> rsnano_nullable_lmdb::Result<RoCursor<'_>> {
        self.inner.open_ro_cursor(database)
    }

    pub fn count(&self, database: LmdbDatabase) -> u64 {
        self.inner.count(database)
    }

    pub fn into_inner(self) -> ReadTransaction {
        self.inner
    }
}

impl LmdbTransaction for WalletReadTxnSHIM {
    fn is_refresh_needed(&self) -> bool {
        self.inner.is_refresh_needed()
    }

    fn is_refresh_needed_with(&self, max_duration: Duration) -> bool {
        self.inner.is_refresh_needed_with(max_duration)
    }

    fn get(&self, database: LmdbDatabase, key: &[u8]) -> rsnano_nullable_lmdb::Result<&[u8]> {
        self.inner.get(database, key)
    }

    fn open_ro_cursor(&self, database: LmdbDatabase) -> rsnano_nullable_lmdb::Result<RoCursor<'_>> {
        self.inner.open_ro_cursor(database)
    }

    fn count(&self, database: LmdbDatabase) -> u64 {
        self.inner.count(database)
    }
}

/// Temporary shim over an LMDB write transaction.
pub struct WalletWriteTxnSHIM {
    inner: WriteTransaction,
}

impl WalletWriteTxnSHIM {
    pub fn new(inner: WriteTransaction) -> Self {
        Self { inner }
    }

    pub fn commit(self) {
        self.inner.commit();
    }

    pub fn put(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: &[u8],
        flags: WriteFlags,
    ) -> rsnano_nullable_lmdb::Result<()> {
        self.inner.put(database, key, value, flags)
    }

    pub fn delete(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> rsnano_nullable_lmdb::Result<()> {
        self.inner.delete(database, key, value)
    }

    pub fn clear_db(&mut self, database: LmdbDatabase) -> rsnano_nullable_lmdb::Result<()> {
        self.inner.clear_db(database)
    }

    pub fn into_inner(self) -> WriteTransaction {
        self.inner
    }
}

impl LmdbTransaction for WalletWriteTxnSHIM {
    fn is_refresh_needed(&self) -> bool {
        self.inner.is_refresh_needed()
    }

    fn is_refresh_needed_with(&self, max_duration: Duration) -> bool {
        self.inner.is_refresh_needed_with(max_duration)
    }

    fn get(&self, database: LmdbDatabase, key: &[u8]) -> rsnano_nullable_lmdb::Result<&[u8]> {
        self.inner.get(database, key)
    }

    fn open_ro_cursor(&self, database: LmdbDatabase) -> rsnano_nullable_lmdb::Result<RoCursor<'_>> {
        self.inner.open_ro_cursor(database)
    }

    fn count(&self, database: LmdbDatabase) -> u64 {
        self.inner.count(database)
    }
}

/// Temporary trait alias letting wallet logic accept either shim without naming LMDB types.
impl WalletReadTxn for WalletReadTxnSHIM {
    fn as_lmdb_txn_shim(&self) -> &dyn LmdbTransaction {
        &self.inner
    }

    fn raw_get(&self, database: LmdbDatabase, key: &[u8]) -> rsnano_nullable_lmdb::Result<&[u8]> {
        self.inner.get(database, key)
    }

    fn raw_open_ro_cursor(
        &self,
        database: LmdbDatabase,
    ) -> rsnano_nullable_lmdb::Result<RoCursor<'_>> {
        self.inner.open_ro_cursor(database)
    }

    fn raw_count(&self, database: LmdbDatabase) -> u64 {
        self.inner.count(database)
    }

    fn commit(self: Box<Self>) {
        WalletReadTxnSHIM::commit(*self);
    }
}

impl WalletReadTxn for WalletWriteTxnSHIM {
    fn as_lmdb_txn_shim(&self) -> &dyn LmdbTransaction {
        &self.inner
    }

    fn raw_get(&self, database: LmdbDatabase, key: &[u8]) -> rsnano_nullable_lmdb::Result<&[u8]> {
        self.inner.get(database, key)
    }

    fn raw_open_ro_cursor(
        &self,
        database: LmdbDatabase,
    ) -> rsnano_nullable_lmdb::Result<RoCursor<'_>> {
        self.inner.open_ro_cursor(database)
    }

    fn raw_count(&self, database: LmdbDatabase) -> u64 {
        self.inner.count(database)
    }

    fn commit(self: Box<Self>) {
        WalletWriteTxnSHIM::commit(*self);
    }
}

impl WalletWriteTxn for WalletWriteTxnSHIM {
    fn as_lmdb_write_txn_shim(&mut self) -> &mut WriteTransaction {
        &mut self.inner
    }

    fn raw_put(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: &[u8],
        flags: WriteFlags,
    ) -> rsnano_nullable_lmdb::Result<()> {
        self.inner.put(database, key, value, flags)
    }

    fn raw_delete(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> rsnano_nullable_lmdb::Result<()> {
        self.inner.delete(database, key, value)
    }

    fn raw_clear_db(&mut self, database: LmdbDatabase) -> rsnano_nullable_lmdb::Result<()> {
        self.inner.clear_db(database)
    }

    fn raw_open_rw_cursor(
        &mut self,
        database: LmdbDatabase,
    ) -> rsnano_nullable_lmdb::Result<RwCursor<'_>> {
        self.inner.open_rw_cursor(database)
    }

    unsafe fn raw_drop_db(&mut self, database: LmdbDatabase) -> rsnano_nullable_lmdb::Result<()> {
        unsafe { self.inner.drop_db(database) }
    }
}

pub trait WalletTxnSHIM: WalletReadTxn + LmdbTransaction {}

impl WalletTxnSHIM for WalletReadTxnSHIM {}
impl WalletTxnSHIM for WalletWriteTxnSHIM {}
