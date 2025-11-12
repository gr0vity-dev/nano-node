use std::time::Duration;

use rsnano_nullable_lmdb::{
    LmdbDatabase, ReadTransaction, RoCursor, Transaction as LmdbTransaction, WriteFlags,
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

    pub fn get(&self, database: LmdbDatabase, key: &[u8]) -> lmdb::Result<&[u8]> {
        self.inner.get(database, key)
    }

    pub fn open_ro_cursor(&self, database: LmdbDatabase) -> lmdb::Result<RoCursor<'_>> {
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

    fn get(&self, database: LmdbDatabase, key: &[u8]) -> lmdb::Result<&[u8]> {
        self.inner.get(database, key)
    }

    fn open_ro_cursor(&self, database: LmdbDatabase) -> lmdb::Result<RoCursor<'_>> {
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
    ) -> lmdb::Result<()> {
        self.inner.put(database, key, value, flags)
    }

    pub fn delete(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> lmdb::Result<()> {
        self.inner.delete(database, key, value)
    }

    pub fn clear_db(&mut self, database: LmdbDatabase) -> lmdb::Result<()> {
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

    fn get(&self, database: LmdbDatabase, key: &[u8]) -> lmdb::Result<&[u8]> {
        self.inner.get(database, key)
    }

    fn open_ro_cursor(&self, database: LmdbDatabase) -> lmdb::Result<RoCursor<'_>> {
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
}

impl WalletReadTxn for WalletWriteTxnSHIM {
    fn as_lmdb_txn_shim(&self) -> &dyn LmdbTransaction {
        &self.inner
    }
}

impl WalletWriteTxn for WalletWriteTxnSHIM {
    fn as_lmdb_write_txn_shim(&mut self) -> &mut WriteTransaction {
        &mut self.inner
    }
}

pub trait WalletTxnSHIM: WalletReadTxn + LmdbTransaction {}

impl WalletTxnSHIM for WalletReadTxnSHIM {}
impl WalletTxnSHIM for WalletWriteTxnSHIM {}
