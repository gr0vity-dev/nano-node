use std::ops::{Deref, DerefMut};
use std::time::Duration;

use crate::LedgerStore;
use rsnano_nullable_lmdb::{
    LmdbDatabase, ReadTransaction, RoCursor, Transaction as LmdbTransaction, WriteTransaction,
};

/// Logic-owned wrapper around `rsnano_nullable_lmdb::ReadTransaction` used by the
/// ledger module. Keeps LMDB specifics out of most call sites while still
/// implementing the underlying `Transaction` trait for store access.
pub struct LedgerReadTransaction {
    inner: ReadTransaction,
}

impl LedgerReadTransaction {
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
}

impl Deref for LedgerReadTransaction {
    type Target = ReadTransaction;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl LmdbTransaction for LedgerReadTransaction {
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

/// Logic-owned wrapper around `WriteTransaction`.
pub struct LedgerWriteTransaction {
    inner: WriteTransaction,
}

impl LedgerWriteTransaction {
    pub fn new(inner: WriteTransaction) -> Self {
        Self { inner }
    }

    pub fn commit(self) {
        self.inner.commit();
    }

    pub fn into_inner(self) -> WriteTransaction {
        self.inner
    }
}

impl Deref for LedgerWriteTransaction {
    type Target = WriteTransaction;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for LedgerWriteTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl LmdbTransaction for LedgerWriteTransaction {
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
pub trait LedgerAnyTransaction: LmdbTransaction {}

impl<T> LedgerAnyTransaction for T where T: LmdbTransaction + ?Sized {}

/// Adapter that turns a borrowed LMDB transaction reference into something that
/// implements `LedgerAnyTransaction` without leaking the LMDB trait to logic
/// call sites. Useful while legacy code still hands around raw `&dyn Transaction`.
pub struct LedgerTransactionAdapter<'a> {
    inner: &'a dyn LmdbTransaction,
}

impl<'a> LedgerTransactionAdapter<'a> {
    pub fn new(inner: &'a dyn LmdbTransaction) -> Self {
        Self { inner }
    }
}

impl LmdbTransaction for LedgerTransactionAdapter<'_> {
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

pub fn begin_read_txn(store: &dyn LedgerStore) -> LedgerReadTransaction {
    LedgerReadTransaction::new(store.begin_read())
}

pub fn begin_write_txn(store: &dyn LedgerStore) -> LedgerWriteTransaction {
    LedgerWriteTransaction::new(store.begin_write())
}

pub fn refresh_write_txn(
    store: &dyn LedgerStore,
    txn: LedgerWriteTransaction,
) -> LedgerWriteTransaction {
    LedgerWriteTransaction::new(store.refresh_write_txn(txn.into_inner()))
}
