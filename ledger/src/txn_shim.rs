use std::time::Duration;

use crate::LedgerStore;
use rsnano_nullable_lmdb::{ReadTransaction, WriteTransaction};
use store_traits::{
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::{
        StoreBackendTransaction, StoreDatabase, StoreResult, StoreRoCursor, StoreRwCursor,
        StoreWriteFlags, StoreWriteTransaction,
    },
};

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
        StoreBackendTransaction::is_refresh_needed(&self.inner)
    }
}

impl StoreBackendTransaction for LedgerReadTxnSHIM {
    fn is_refresh_needed(&self) -> bool {
        StoreBackendTransaction::is_refresh_needed(&self.inner)
    }

    fn is_refresh_needed_with(&self, max_duration: Duration) -> bool {
        self.inner.is_refresh_needed_with(max_duration)
    }

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        StoreBackendTransaction::get(&self.inner, database, key)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        StoreBackendTransaction::open_ro_cursor(&self.inner, database)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        StoreBackendTransaction::count(&self.inner, database)
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
        StoreBackendTransaction::is_refresh_needed(&self.inner)
    }
}

impl StoreBackendTransaction for LedgerWriteTxnSHIM {
    fn is_refresh_needed(&self) -> bool {
        StoreBackendTransaction::is_refresh_needed(&self.inner)
    }

    fn is_refresh_needed_with(&self, max_duration: Duration) -> bool {
        self.inner.is_refresh_needed_with(max_duration)
    }

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        StoreBackendTransaction::get(&self.inner, database, key)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        StoreBackendTransaction::open_ro_cursor(&self.inner, database)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        StoreBackendTransaction::count(&self.inner, database)
    }
}

/// Shared transaction trait exposed inside the ledger module so logic code can
/// accept "any" ledger transaction without importing LMDB types.
impl LedgerReadTxn for LedgerReadTxnSHIM {
    fn is_refresh_needed(&self) -> bool {
        self.is_refresh_needed()
    }

    fn as_lmdb_txn_shim(&self) -> &dyn StoreBackendTransaction {
        &self.inner
    }

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        StoreBackendTransaction::get(&self.inner, database, key)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        StoreBackendTransaction::open_ro_cursor(&self.inner, database)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        StoreBackendTransaction::count(&self.inner, database)
    }
}

impl LedgerReadTxn for LedgerWriteTxnSHIM {
    fn is_refresh_needed(&self) -> bool {
        self.is_refresh_needed()
    }

    fn as_lmdb_txn_shim(&self) -> &dyn StoreBackendTransaction {
        &self.inner
    }

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        StoreBackendTransaction::get(&self.inner, database, key)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        StoreBackendTransaction::open_ro_cursor(&self.inner, database)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        StoreBackendTransaction::count(&self.inner, database)
    }
}

impl LedgerWriteTxn for LedgerWriteTxnSHIM {
    fn as_lmdb_write_txn_shim(&mut self) -> StoreWriteTransaction<'_> {
        StoreWriteTransaction::new(&mut self.inner)
    }

    fn put(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: &[u8],
        flags: StoreWriteFlags,
    ) -> StoreResult<()> {
        self.inner
            .put(database.into(), key, value, flags.into())
            .map_err(Into::into)
    }

    fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()> {
        self.inner
            .delete(database.into(), key, value)
            .map_err(Into::into)
    }

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        self.inner.clear_db(database.into()).map_err(Into::into)
    }

    fn open_rw_cursor(&mut self, database: StoreDatabase) -> StoreResult<StoreRwCursor<'_>> {
        self.inner
            .open_rw_cursor(database.into())
            .map(StoreRwCursor::new)
            .map_err(Into::into)
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        unsafe { self.inner.drop_db(database.into()) }.map_err(Into::into)
    }
}

pub trait LedgerTxnSHIM: LedgerReadTxn + StoreBackendTransaction {}

impl<T> LedgerTxnSHIM for T where T: LedgerReadTxn + StoreBackendTransaction + ?Sized {}

/// Adapter that turns a borrowed LMDB transaction reference into something that
/// implements `LedgerTxnSHIM` without leaking the LMDB trait to logic
/// call sites. Useful while legacy code still hands around raw `&dyn Transaction`.
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
