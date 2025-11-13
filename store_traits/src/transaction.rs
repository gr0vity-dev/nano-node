use rsnano_nullable_lmdb::{
    Error as LmdbError, LmdbDatabase, Result as LmdbResult, RoCursor, RwCursor,
    Transaction as LmdbTransaction, WriteFlags, WriteTransaction,
};

pub trait LedgerReadTxn {
    fn is_refresh_needed(&self) -> bool;
    /// Temporary LMDB escape hatch until adapters are in place.
    fn as_lmdb_txn_shim(&self) -> &dyn rsnano_nullable_lmdb::Transaction;
    fn raw_get(&self, database: LmdbDatabase, key: &[u8]) -> LmdbResult<&[u8]>;
    fn raw_exists(&self, database: LmdbDatabase, key: &[u8]) -> bool {
        match self.raw_get(database, key) {
            Ok(_) => true,
            Err(LmdbError::NotFound) => false,
            Err(e) => panic!("exists failed: {:?}", e),
        }
    }
    fn raw_open_ro_cursor(&self, database: LmdbDatabase) -> LmdbResult<RoCursor<'_>>;
    fn raw_count(&self, database: LmdbDatabase) -> u64;
}

pub trait LedgerWriteTxn: LedgerReadTxn {
    fn as_lmdb_write_txn_shim(&mut self) -> &mut rsnano_nullable_lmdb::WriteTransaction;
    fn raw_put(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: &[u8],
        flags: WriteFlags,
    ) -> LmdbResult<()>;
    fn raw_delete(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> LmdbResult<()>;
    fn raw_clear_db(&mut self, database: LmdbDatabase) -> LmdbResult<()>;
    fn raw_open_rw_cursor(&mut self, database: LmdbDatabase) -> LmdbResult<RwCursor<'_>>;
    unsafe fn raw_drop_db(&mut self, database: LmdbDatabase) -> LmdbResult<()>;
}

impl LedgerReadTxn for rsnano_nullable_lmdb::ReadTransaction {
    fn is_refresh_needed(&self) -> bool {
        LmdbTransaction::is_refresh_needed(self)
    }

    fn as_lmdb_txn_shim(&self) -> &dyn rsnano_nullable_lmdb::Transaction {
        self
    }

    fn raw_get(&self, database: LmdbDatabase, key: &[u8]) -> LmdbResult<&[u8]> {
        LmdbTransaction::get(self, database, key)
    }

    fn raw_open_ro_cursor(&self, database: LmdbDatabase) -> LmdbResult<RoCursor<'_>> {
        LmdbTransaction::open_ro_cursor(self, database)
    }

    fn raw_count(&self, database: LmdbDatabase) -> u64 {
        LmdbTransaction::count(self, database)
    }
}

impl LedgerReadTxn for rsnano_nullable_lmdb::WriteTransaction {
    fn is_refresh_needed(&self) -> bool {
        LmdbTransaction::is_refresh_needed(self)
    }

    fn as_lmdb_txn_shim(&self) -> &dyn rsnano_nullable_lmdb::Transaction {
        self
    }

    fn raw_get(&self, database: LmdbDatabase, key: &[u8]) -> LmdbResult<&[u8]> {
        LmdbTransaction::get(self, database, key)
    }

    fn raw_open_ro_cursor(&self, database: LmdbDatabase) -> LmdbResult<RoCursor<'_>> {
        LmdbTransaction::open_ro_cursor(self, database)
    }

    fn raw_count(&self, database: LmdbDatabase) -> u64 {
        LmdbTransaction::count(self, database)
    }
}

impl LedgerWriteTxn for rsnano_nullable_lmdb::WriteTransaction {
    fn as_lmdb_write_txn_shim(&mut self) -> &mut rsnano_nullable_lmdb::WriteTransaction {
        self
    }

    fn raw_put(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: &[u8],
        flags: WriteFlags,
    ) -> LmdbResult<()> {
        WriteTransaction::put(self, database, key, value, flags)
    }

    fn raw_delete(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> LmdbResult<()> {
        WriteTransaction::delete(self, database, key, value)
    }

    fn raw_clear_db(&mut self, database: LmdbDatabase) -> LmdbResult<()> {
        WriteTransaction::clear_db(self, database)
    }

    fn raw_open_rw_cursor(&mut self, database: LmdbDatabase) -> LmdbResult<RwCursor<'_>> {
        WriteTransaction::open_rw_cursor(self, database)
    }

    unsafe fn raw_drop_db(&mut self, database: LmdbDatabase) -> LmdbResult<()> {
        unsafe { WriteTransaction::drop_db(self, database) }
    }
}

pub trait WalletReadTxn {
    fn get(&self, database: LmdbDatabase, key: &[u8]) -> LmdbResult<&[u8]>;
    fn open_ro_cursor(&self, database: LmdbDatabase) -> LmdbResult<RoCursor<'_>>;
    fn count(&self, database: LmdbDatabase) -> u64;
    fn commit(self: Box<Self>);
}

pub trait WalletWriteTxn: WalletReadTxn {
    fn put(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: &[u8],
        flags: WriteFlags,
    ) -> LmdbResult<()>;
    fn delete(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> LmdbResult<()>;
    fn clear_db(&mut self, database: LmdbDatabase) -> LmdbResult<()>;
    fn open_rw_cursor(&mut self, database: LmdbDatabase) -> LmdbResult<RwCursor<'_>>;
    unsafe fn drop_db(&mut self, database: LmdbDatabase) -> LmdbResult<()>;
}

impl WalletReadTxn for rsnano_nullable_lmdb::ReadTransaction {
    fn get(&self, database: LmdbDatabase, key: &[u8]) -> LmdbResult<&[u8]> {
        LmdbTransaction::get(self, database, key)
    }

    fn open_ro_cursor(&self, database: LmdbDatabase) -> LmdbResult<RoCursor<'_>> {
        LmdbTransaction::open_ro_cursor(self, database)
    }

    fn count(&self, database: LmdbDatabase) -> u64 {
        LmdbTransaction::count(self, database)
    }

    fn commit(self: Box<Self>) {
        (*self).commit();
    }
}

impl WalletReadTxn for rsnano_nullable_lmdb::WriteTransaction {
    fn get(&self, database: LmdbDatabase, key: &[u8]) -> LmdbResult<&[u8]> {
        LmdbTransaction::get(self, database, key)
    }

    fn open_ro_cursor(&self, database: LmdbDatabase) -> LmdbResult<RoCursor<'_>> {
        LmdbTransaction::open_ro_cursor(self, database)
    }

    fn count(&self, database: LmdbDatabase) -> u64 {
        LmdbTransaction::count(self, database)
    }

    fn commit(self: Box<Self>) {
        (*self).commit();
    }
}

impl WalletWriteTxn for rsnano_nullable_lmdb::WriteTransaction {
    fn put(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: &[u8],
        flags: WriteFlags,
    ) -> LmdbResult<()> {
        WriteTransaction::put(self, database, key, value, flags)
    }

    fn delete(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> LmdbResult<()> {
        WriteTransaction::delete(self, database, key, value)
    }

    fn clear_db(&mut self, database: LmdbDatabase) -> LmdbResult<()> {
        WriteTransaction::clear_db(self, database)
    }

    fn open_rw_cursor(&mut self, database: LmdbDatabase) -> LmdbResult<RwCursor<'_>> {
        WriteTransaction::open_rw_cursor(self, database)
    }

    unsafe fn drop_db(&mut self, database: LmdbDatabase) -> LmdbResult<()> {
        unsafe { WriteTransaction::drop_db(self, database) }
    }
}
