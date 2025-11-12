use std::ops::{Deref, DerefMut};
use std::time::Duration;

use crate::LedgerStore;
use rsnano_nullable_lmdb::{
    LmdbDatabase, ReadTransaction, RoCursor, Transaction as LmdbTransaction, WriteTransaction,
};
use store_traits::transaction::{LedgerReadTxn, LedgerWriteTxn};

/// Temporary shim over `rsnano_nullable_lmdb::ReadTransaction` for ledger logic.
pub struct LedgerReadTxnSHIM {
    inner: ReadTransaction,
}

impl LedgerReadTxnSHIM {
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

    pub fn into_inner(self) -> ReadTransaction {
        self.inner
    }

    pub fn is_refresh_needed(&self) -> bool {
        self.inner.is_refresh_needed()
    }
}

impl Deref for LedgerReadTxnSHIM {
    type Target = ReadTransaction;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl LmdbTransaction for LedgerReadTxnSHIM {
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

/// Temporary shim over `WriteTransaction` for ledger logic.
pub struct LedgerWriteTxnSHIM {
    inner: WriteTransaction,
}

impl LedgerWriteTxnSHIM {
    pub fn new(inner: WriteTransaction) -> Self {
        Self { inner }
    }

    pub fn commit(self) {
        self.inner.commit();
    }

    pub fn into_inner(self) -> WriteTransaction {
        self.inner
    }

    pub fn is_refresh_needed(&self) -> bool {
        self.inner.is_refresh_needed()
    }
}

impl Deref for LedgerWriteTxnSHIM {
    type Target = WriteTransaction;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for LedgerWriteTxnSHIM {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl LmdbTransaction for LedgerWriteTxnSHIM {
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

/// Shared transaction trait exposed inside the ledger module so logic code can
/// accept "any" ledger transaction without importing LMDB types.
impl LedgerReadTxn for LedgerReadTxnSHIM {
    fn as_lmdb_txn_shim(&self) -> &dyn LmdbTransaction {
        &self.inner
    }
}

impl LedgerReadTxn for LedgerWriteTxnSHIM {
    fn as_lmdb_txn_shim(&self) -> &dyn LmdbTransaction {
        &self.inner
    }
}

impl LedgerWriteTxn for LedgerWriteTxnSHIM {
    fn as_lmdb_write_txn_shim(&mut self) -> &mut WriteTransaction {
        &mut self.inner
    }
}

pub trait LedgerTxnSHIM: LedgerReadTxn + LmdbTransaction {}

impl<T> LedgerTxnSHIM for T where T: LedgerReadTxn + LmdbTransaction + ?Sized {}

/// Adapter that turns a borrowed LMDB transaction reference into something that
/// implements `LedgerTxnSHIM` without leaking the LMDB trait to logic
/// call sites. Useful while legacy code still hands around raw `&dyn Transaction`.
pub struct LedgerTxnAdapterSHIM<'a> {
    inner: &'a dyn LedgerTxnSHIM,
}

impl<'a> LedgerTxnAdapterSHIM<'a> {
    pub fn new(inner: &'a dyn LedgerTxnSHIM) -> Self {
        Self { inner }
    }
}

impl LmdbTransaction for LedgerTxnAdapterSHIM<'_> {
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

impl<'a> LedgerReadTxn for LedgerTxnAdapterSHIM<'a> {
    fn as_lmdb_txn_shim(&self) -> &dyn LmdbTransaction {
        self.inner.as_lmdb_txn_shim()
    }
}

#[allow(non_snake_case)]
pub fn begin_read_txn_SHIM(store: &dyn LedgerStore) -> LedgerReadTxnSHIM {
    LedgerReadTxnSHIM::new(store.begin_read())
}

#[allow(non_snake_case)]
pub fn begin_write_txn_SHIM(store: &dyn LedgerStore) -> LedgerWriteTxnSHIM {
    LedgerWriteTxnSHIM::new(store.begin_write())
}

#[allow(non_snake_case)]
pub fn refresh_write_txn_SHIM(
    store: &dyn LedgerStore,
    txn: LedgerWriteTxnSHIM,
) -> LedgerWriteTxnSHIM {
    LedgerWriteTxnSHIM::new(store.refresh_write_txn(txn.into_inner()))
}
