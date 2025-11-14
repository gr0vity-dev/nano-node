use rsnano_nullable_lmdb::{ReadTransaction, Transaction as LmdbTxn, WriteTransaction};
use store_traits::transaction::{LedgerReadTxn, LedgerWriteTxn, WalletReadTxn, WalletWriteTxn};
use store_traits::types::{
    StoreDatabase, StoreError, StoreResult, StoreRoCursor, StoreRwCursor, StoreWriteFlags,
};

use crate::store_utils::{
    lmdb_write_flags_from, store_ro_cursor_from_lmdb, store_rw_cursor_from_lmdb,
};

pub struct LmdbLedgerReadTxn {
    inner: ReadTransaction,
}

impl LmdbLedgerReadTxn {
    pub fn new(inner: ReadTransaction) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> ReadTransaction {
        self.inner
    }

    pub fn as_inner(&self) -> &ReadTransaction {
        &self.inner
    }

    pub fn commit(self) {
        self.inner.commit();
    }
}

impl LedgerReadTxn for LmdbLedgerReadTxn {
    fn is_refresh_needed(&self) -> bool {
        LmdbTxn::is_refresh_needed(&self.inner)
    }

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        LmdbTxn::get(&self.inner, database.into(), key).map_err(Into::into)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        LmdbTxn::open_ro_cursor(&self.inner, database.into())
            .map(store_ro_cursor_from_lmdb)
            .map_err(Into::into)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        LmdbTxn::count(&self.inner, database.into())
    }
}

pub struct LmdbLedgerWriteTxn {
    inner: WriteTransaction,
}

impl LmdbLedgerWriteTxn {
    pub fn new(inner: WriteTransaction) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> WriteTransaction {
        self.inner
    }

    pub fn as_inner(&self) -> &WriteTransaction {
        &self.inner
    }

    pub fn as_inner_mut(&mut self) -> &mut WriteTransaction {
        &mut self.inner
    }

    pub fn commit(self) {
        self.inner.commit();
    }
}

impl LedgerReadTxn for LmdbLedgerWriteTxn {
    fn is_refresh_needed(&self) -> bool {
        LmdbTxn::is_refresh_needed(&self.inner)
    }

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        LmdbTxn::get(&self.inner, database.into(), key).map_err(Into::into)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        LmdbTxn::open_ro_cursor(&self.inner, database.into())
            .map(store_ro_cursor_from_lmdb)
            .map_err(Into::into)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        LmdbTxn::count(&self.inner, database.into())
    }
}

impl LedgerWriteTxn for LmdbLedgerWriteTxn {
    fn put(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: &[u8],
        flags: StoreWriteFlags,
    ) -> StoreResult<()> {
        self.inner
            .put(database.into(), key, value, lmdb_write_flags_from(flags))
            .map_err(StoreError::from)
    }

    fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()> {
        self.inner
            .delete(database.into(), key, value)
            .map_err(StoreError::from)
    }

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        self.inner
            .clear_db(database.into())
            .map_err(StoreError::from)
    }

    fn open_rw_cursor(&mut self, database: StoreDatabase) -> StoreResult<StoreRwCursor<'_>> {
        self.inner
            .open_rw_cursor(database.into())
            .map(store_rw_cursor_from_lmdb)
            .map_err(StoreError::from)
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        unsafe { self.inner.drop_db(database.into()) }.map_err(StoreError::from)
    }

    fn commit(self: Box<Self>) {
        self.inner.commit();
    }
}

impl WalletReadTxn for LmdbLedgerReadTxn {
    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        LmdbTxn::get(&self.inner, database.into(), key).map_err(Into::into)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        LmdbTxn::open_ro_cursor(&self.inner, database.into())
            .map(store_ro_cursor_from_lmdb)
            .map_err(Into::into)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        LmdbTxn::count(&self.inner, database.into())
    }

    fn commit(self: Box<Self>) {
        self.inner.commit();
    }
}

impl WalletReadTxn for LmdbLedgerWriteTxn {
    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        LmdbTxn::get(&self.inner, database.into(), key).map_err(Into::into)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        LmdbTxn::open_ro_cursor(&self.inner, database.into())
            .map(store_ro_cursor_from_lmdb)
            .map_err(Into::into)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        LmdbTxn::count(&self.inner, database.into())
    }

    fn commit(self: Box<Self>) {
        self.inner.commit();
    }
}

impl WalletWriteTxn for LmdbLedgerWriteTxn {
    fn put(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: &[u8],
        flags: StoreWriteFlags,
    ) -> StoreResult<()> {
        self.inner
            .put(database.into(), key, value, lmdb_write_flags_from(flags))
            .map_err(StoreError::from)
    }

    fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()> {
        self.inner
            .delete(database.into(), key, value)
            .map_err(StoreError::from)
    }

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        self.inner
            .clear_db(database.into())
            .map_err(StoreError::from)
    }

    fn open_rw_cursor(&mut self, database: StoreDatabase) -> StoreResult<StoreRwCursor<'_>> {
        self.inner
            .open_rw_cursor(database.into())
            .map(store_rw_cursor_from_lmdb)
            .map_err(StoreError::from)
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        unsafe { self.inner.drop_db(database.into()) }.map_err(StoreError::from)
    }
}
