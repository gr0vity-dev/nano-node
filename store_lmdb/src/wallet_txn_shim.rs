use rsnano_nullable_lmdb::{ReadTransaction, Transaction as LmdbTransaction, WriteTransaction};
use store_traits::{
    transaction::{WalletReadTxn, WalletWriteTxn},
    types::{StoreDatabase, StoreResult, StoreRoCursor, StoreRwCursor, StoreWriteFlags},
};

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

    pub fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        LmdbTransaction::get(&self.inner, database.into(), key).map_err(Into::into)
    }

    pub fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        LmdbTransaction::open_ro_cursor(&self.inner, database.into())
            .map(StoreRoCursor::new)
            .map_err(Into::into)
    }

    pub fn count(&self, database: StoreDatabase) -> u64 {
        LmdbTransaction::count(&self.inner, database.into())
    }

    pub fn into_inner(self) -> ReadTransaction {
        self.inner
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
        database: StoreDatabase,
        key: &[u8],
        value: &[u8],
        flags: StoreWriteFlags,
    ) -> StoreResult<()> {
        self.inner
            .put(database.into(), key, value, flags.into())
            .map_err(Into::into)
    }

    pub fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()> {
        self.inner
            .delete(database.into(), key, value)
            .map_err(Into::into)
    }

    pub fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        self.inner.clear_db(database.into()).map_err(Into::into)
    }

    pub fn open_rw_cursor(&mut self, database: StoreDatabase) -> StoreResult<StoreRwCursor<'_>> {
        self.inner
            .open_rw_cursor(database.into())
            .map(StoreRwCursor::new)
            .map_err(Into::into)
    }

    pub unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        unsafe { self.inner.drop_db(database.into()) }.map_err(Into::into)
    }

    pub fn into_inner(self) -> WriteTransaction {
        self.inner
    }
}

impl WalletReadTxn for WalletReadTxnSHIM {
    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        WalletReadTxnSHIM::get(self, database, key)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        WalletReadTxnSHIM::open_ro_cursor(self, database)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        WalletReadTxnSHIM::count(self, database)
    }

    fn commit(self: Box<Self>) {
        WalletReadTxnSHIM::commit(*self);
    }
}

impl WalletReadTxn for WalletWriteTxnSHIM {
    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        LmdbTransaction::get(&self.inner, database.into(), key).map_err(Into::into)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        LmdbTransaction::open_ro_cursor(&self.inner, database.into())
            .map(StoreRoCursor::new)
            .map_err(Into::into)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        LmdbTransaction::count(&self.inner, database.into())
    }

    fn commit(self: Box<Self>) {
        WalletWriteTxnSHIM::commit(*self);
    }
}

impl WalletWriteTxn for WalletWriteTxnSHIM {
    fn put(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: &[u8],
        flags: StoreWriteFlags,
    ) -> StoreResult<()> {
        WalletWriteTxnSHIM::put(self, database, key, value, flags)
    }

    fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()> {
        WalletWriteTxnSHIM::delete(self, database, key, value)
    }

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        WalletWriteTxnSHIM::clear_db(self, database)
    }

    fn open_rw_cursor(&mut self, database: StoreDatabase) -> StoreResult<StoreRwCursor<'_>> {
        WalletWriteTxnSHIM::open_rw_cursor(self, database)
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        unsafe { WalletWriteTxnSHIM::drop_db(self, database) }
    }
}
