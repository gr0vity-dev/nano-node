use std::{
    ops::{Deref, DerefMut},
    time::Duration,
};

use rsnano_nullable_lmdb::{
    LmdbDatabase, ReadTransaction, RoCursor, Transaction as LmdbTransaction, WriteFlags,
    WriteTransaction,
};

/// Logic-owned view over an LMDB read transaction. Provides only the operations
/// wallet code needs while still implementing the LMDB `Transaction` trait so it
/// can be handed to store abstractions without exposing the raw type.
pub struct WalletReadTransaction {
    inner: ReadTransaction,
}

impl WalletReadTransaction {
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

impl Deref for WalletReadTransaction {
    type Target = ReadTransaction;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl LmdbTransaction for WalletReadTransaction {
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

/// Logic-owned view over an LMDB write transaction.
pub struct WalletWriteTransaction {
    inner: WriteTransaction,
}

impl WalletWriteTransaction {
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

impl Deref for WalletWriteTransaction {
    type Target = WriteTransaction;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for WalletWriteTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl LmdbTransaction for WalletWriteTransaction {
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

/// Shared bound for logic code that only needs "transaction-like" behavior
/// without importing the LMDB trait everywhere.
pub trait WalletAnyTransaction: LmdbTransaction {}

impl WalletAnyTransaction for WalletReadTransaction {}
impl WalletAnyTransaction for WalletWriteTransaction {}
