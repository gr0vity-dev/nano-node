use crate::types::{
    StoreBackendTransaction, StoreDatabase, StoreError, StoreResult, StoreRoCursor, StoreRwCursor,
    StoreWriteFlags, StoreWriteTransaction,
};

pub trait LedgerReadTxn {
    fn is_refresh_needed(&self) -> bool;
    /// Temporary LMDB escape hatch until adapters are in place.
    fn as_lmdb_txn_shim(&self) -> &dyn StoreBackendTransaction;
    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]>;
    fn raw_exists(&self, database: StoreDatabase, key: &[u8]) -> bool {
        match self.get(database, key) {
            Ok(_) => true,
            Err(StoreError::NotFound) => false,
            Err(e) => panic!("exists failed: {:?}", e),
        }
    }
    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>>;
    fn count(&self, database: StoreDatabase) -> u64;
    fn raw_get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        self.get(database, key)
    }
    fn raw_open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        self.open_ro_cursor(database)
    }
    fn raw_count(&self, database: StoreDatabase) -> u64 {
        self.count(database)
    }
}

pub trait LedgerWriteTxn: LedgerReadTxn {
    fn as_lmdb_write_txn_shim(&mut self) -> &mut StoreWriteTransaction;
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

impl LedgerReadTxn for rsnano_nullable_lmdb::ReadTransaction {
    fn is_refresh_needed(&self) -> bool {
        StoreBackendTransaction::is_refresh_needed(self)
    }

    fn as_lmdb_txn_shim(&self) -> &dyn rsnano_nullable_lmdb::Transaction {
        self
    }

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        StoreBackendTransaction::get(self, database, key)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        StoreBackendTransaction::open_ro_cursor(self, database)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        StoreBackendTransaction::count(self, database)
    }
}

impl LedgerReadTxn for rsnano_nullable_lmdb::WriteTransaction {
    fn is_refresh_needed(&self) -> bool {
        StoreBackendTransaction::is_refresh_needed(self)
    }

    fn as_lmdb_txn_shim(&self) -> &dyn rsnano_nullable_lmdb::Transaction {
        self
    }

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        StoreBackendTransaction::get(self, database, key)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        StoreBackendTransaction::open_ro_cursor(self, database)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        StoreBackendTransaction::count(self, database)
    }
}

impl LedgerWriteTxn for rsnano_nullable_lmdb::WriteTransaction {
    fn as_lmdb_write_txn_shim(&mut self) -> &mut rsnano_nullable_lmdb::WriteTransaction {
        self
    }

    fn put(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: &[u8],
        flags: StoreWriteFlags,
    ) -> StoreResult<()> {
        StoreWriteTransaction::put(self, database, key, value, flags)
    }

    fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()> {
        StoreWriteTransaction::delete(self, database, key, value)
    }

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        StoreWriteTransaction::clear_db(self, database)
    }

    fn open_rw_cursor(&mut self, database: StoreDatabase) -> StoreResult<StoreRwCursor<'_>> {
        StoreWriteTransaction::open_rw_cursor(self, database)
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        unsafe { StoreWriteTransaction::drop_db(self, database) }
    }
}

pub trait WalletReadTxn {
    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]>;
    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>>;
    fn count(&self, database: StoreDatabase) -> u64;
    fn commit(self: Box<Self>);
}

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

impl WalletReadTxn for rsnano_nullable_lmdb::ReadTransaction {
    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        StoreBackendTransaction::get(self, database, key)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        StoreBackendTransaction::open_ro_cursor(self, database)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        StoreBackendTransaction::count(self, database)
    }

    fn commit(self: Box<Self>) {
        (*self).commit();
    }
}

impl WalletReadTxn for rsnano_nullable_lmdb::WriteTransaction {
    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        StoreBackendTransaction::get(self, database, key)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        StoreBackendTransaction::open_ro_cursor(self, database)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        StoreBackendTransaction::count(self, database)
    }

    fn commit(self: Box<Self>) {
        (*self).commit();
    }
}

impl WalletWriteTxn for rsnano_nullable_lmdb::WriteTransaction {
    fn put(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: &[u8],
        flags: StoreWriteFlags,
    ) -> StoreResult<()> {
        StoreWriteTransaction::put(self, database, key, value, flags)
    }

    fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()> {
        StoreWriteTransaction::delete(self, database, key, value)
    }

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        StoreWriteTransaction::clear_db(self, database)
    }

    fn open_rw_cursor(&mut self, database: StoreDatabase) -> StoreResult<StoreRwCursor<'_>> {
        StoreWriteTransaction::open_rw_cursor(self, database)
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        unsafe { StoreWriteTransaction::drop_db(self, database) }
    }
}
