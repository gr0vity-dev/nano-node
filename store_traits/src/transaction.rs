use crate::types::{
    StoreDatabase, StoreResult, StoreRoCursor, StoreRwCursor, StoreValue, StoreWriteFlags,
};

/// Transaction interface exposed to ledger components for read-only access.
pub trait LedgerReadTxn {
    fn is_refresh_needed(&self) -> bool;
    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<StoreValue>;

    fn raw_exists(&self, database: StoreDatabase, key: &[u8]) -> bool {
        match self.get(database, key) {
            Ok(_) => true,
            Err(e) if e.is_not_found() => false,
            Err(e) => panic!("exists failed: {:?}", e),
        }
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>>;
    fn count(&self, database: StoreDatabase) -> StoreResult<u64>;

    fn raw_get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<StoreValue> {
        self.get(database, key)
    }

    fn raw_open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        self.open_ro_cursor(database)
    }

    fn raw_count(&self, database: StoreDatabase) -> u64 {
        self.count(database)
            .unwrap_or_else(|e| panic!("count failed: {:?}", e))
    }
}

/// Write-capable ledger transaction.
pub trait LedgerWriteTxn: LedgerReadTxn {
    fn put(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: &[u8],
        flags: StoreWriteFlags,
    ) -> StoreResult<()>;

    fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()>;

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()>;

    fn open_rw_cursor(&mut self, database: StoreDatabase) -> StoreResult<StoreRwCursor<'_>>;

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()>;

    fn commit(self: Box<Self>) -> StoreResult<()>;

    fn raw_put(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: &[u8],
        flags: StoreWriteFlags,
    ) -> StoreResult<()> {
        self.put(database, key, value, flags)
    }

    fn raw_delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()> {
        self.delete(database, key, value)
    }

    fn raw_clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        self.clear_db(database)
    }

    fn raw_open_rw_cursor(&mut self, database: StoreDatabase) -> StoreResult<StoreRwCursor<'_>> {
        self.open_rw_cursor(database)
    }

    unsafe fn raw_drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        unsafe { self.drop_db(database) }
    }
}

/// Wallet-specific read transaction surface.
pub trait WalletReadTxn {
    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<StoreValue>;
    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>>;
    fn count(&self, database: StoreDatabase) -> StoreResult<u64>;
    fn commit(self: Box<Self>) -> StoreResult<()>;
}

/// Wallet write transaction built on top of the read surface.
pub trait WalletWriteTxn: WalletReadTxn {
    fn put(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: &[u8],
        flags: StoreWriteFlags,
    ) -> StoreResult<()>;

    fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()>;

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()>;

    fn open_rw_cursor(&mut self, database: StoreDatabase) -> StoreResult<StoreRwCursor<'_>>;

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()>;
}
