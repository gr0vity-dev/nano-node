use std::num::NonZeroUsize;

use crate::types::{
    StoreDatabase, StoreError, StoreResult, StoreRoCursor, StoreRwCursor, StoreWriteFlags,
};
use rsnano_nullable_lmdb::{
    LmdbDatabase, RoCursor as LmdbRoCursor, RwCursor as LmdbRwCursor, Transaction as LmdbTxn,
    WriteFlags,
};

fn store_ro_cursor_from_lmdb<'txn>(cursor: LmdbRoCursor<'txn>) -> StoreRoCursor<'txn> {
    let raw = Box::into_raw(Box::new(cursor)) as usize;
    let handle = unsafe { NonZeroUsize::new_unchecked(raw) };
    unsafe { StoreRoCursor::from_raw_parts(handle, drop_lmdb_ro_cursor) }
}

fn store_rw_cursor_from_lmdb<'txn>(cursor: LmdbRwCursor<'txn>) -> StoreRwCursor<'txn> {
    let raw = Box::into_raw(Box::new(cursor)) as usize;
    let handle = unsafe { NonZeroUsize::new_unchecked(raw) };
    unsafe { StoreRwCursor::from_raw_parts(handle, drop_lmdb_rw_cursor) }
}

fn store_ro_cursor_into_lmdb<'txn>(cursor: StoreRoCursor<'txn>) -> LmdbRoCursor<'txn> {
    let (handle, _) = cursor.into_raw_parts();
    let ptr = handle.get() as *mut LmdbRoCursor<'txn>;
    *unsafe { Box::from_raw(ptr) }
}

fn store_rw_cursor_into_lmdb<'txn>(cursor: StoreRwCursor<'txn>) -> LmdbRwCursor<'txn> {
    let (handle, _) = cursor.into_raw_parts();
    let ptr = handle.get() as *mut LmdbRwCursor<'txn>;
    *unsafe { Box::from_raw(ptr) }
}

unsafe fn drop_lmdb_ro_cursor(handle: NonZeroUsize) {
    let ptr = handle.get() as *mut LmdbRoCursor<'static>;
    drop(unsafe { Box::from_raw(ptr) });
}

unsafe fn drop_lmdb_rw_cursor(handle: NonZeroUsize) {
    let ptr = handle.get() as *mut LmdbRwCursor<'static>;
    drop(unsafe { Box::from_raw(ptr) });
}

fn to_lmdb_write_flags(flags: StoreWriteFlags) -> WriteFlags {
    WriteFlags::from_bits_truncate(flags.bits())
}

fn from_lmdb_write_flags(flags: WriteFlags) -> StoreWriteFlags {
    StoreWriteFlags::from_bits(flags.bits())
}

pub trait LedgerReadTxn {
    fn is_refresh_needed(&self) -> bool;
    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]>;
    fn raw_exists(&self, database: StoreDatabase, key: &[u8]) -> bool {
        match self.get(database, key) {
            Ok(_) => true,
            Err(e) if e.is_not_found() => false,
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
    fn commit(self: Box<Self>);
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
        LmdbTxn::is_refresh_needed(self)
    }

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        LmdbTxn::get(self, database.into(), key).map_err(Into::into)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        LmdbTxn::open_ro_cursor(self, database.into())
            .map(store_ro_cursor_from_lmdb)
            .map_err(Into::into)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        LmdbTxn::count(self, database.into())
    }
}

impl LedgerReadTxn for rsnano_nullable_lmdb::WriteTransaction {
    fn is_refresh_needed(&self) -> bool {
        LmdbTxn::is_refresh_needed(self)
    }

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        LmdbTxn::get(self, database.into(), key).map_err(Into::into)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        LmdbTxn::open_ro_cursor(self, database.into())
            .map(store_ro_cursor_from_lmdb)
            .map_err(Into::into)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        LmdbTxn::count(self, database.into())
    }
}

impl LedgerWriteTxn for rsnano_nullable_lmdb::WriteTransaction {
    fn put(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: &[u8],
        flags: StoreWriteFlags,
    ) -> StoreResult<()> {
        self.put(database.into(), key, value, to_lmdb_write_flags(flags))
            .map_err(StoreError::from)
    }

    fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()> {
        self.delete(database.into(), key, value)
            .map_err(StoreError::from)
    }

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        self.clear_db(database.into()).map_err(StoreError::from)
    }

    fn open_rw_cursor(&mut self, database: StoreDatabase) -> StoreResult<StoreRwCursor<'_>> {
        self.open_rw_cursor(database.into())
            .map(store_rw_cursor_from_lmdb)
            .map_err(StoreError::from)
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        unsafe { self.drop_db(database.into()) }.map_err(StoreError::from)
    }

    fn commit(self: Box<Self>) {
        (*self).commit();
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

pub trait WalletReadTxnLmdbExt {
    fn get_lmdb(&self, database: LmdbDatabase, key: &[u8]) -> StoreResult<&[u8]>;
    fn open_ro_cursor_lmdb(&self, database: LmdbDatabase) -> StoreResult<LmdbRoCursor<'_>>;
    fn count_lmdb(&self, database: LmdbDatabase) -> u64;
}

impl<T: WalletReadTxn + ?Sized> WalletReadTxnLmdbExt for T {
    fn get_lmdb(&self, database: LmdbDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        self.get(database.into(), key)
    }

    fn open_ro_cursor_lmdb(&self, database: LmdbDatabase) -> StoreResult<LmdbRoCursor<'_>> {
        self.open_ro_cursor(database.into())
            .map(store_ro_cursor_into_lmdb)
    }

    fn count_lmdb(&self, database: LmdbDatabase) -> u64 {
        self.count(database.into())
    }
}

pub trait WalletWriteTxnLmdbExt: WalletReadTxnLmdbExt {
    fn put_lmdb(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: &[u8],
        flags: WriteFlags,
    ) -> StoreResult<()>;
    fn delete_lmdb(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()>;
    fn clear_db_lmdb(&mut self, database: LmdbDatabase) -> StoreResult<()>;
    fn open_rw_cursor_lmdb(&mut self, database: LmdbDatabase) -> StoreResult<LmdbRwCursor<'_>>;
    unsafe fn drop_db_lmdb(&mut self, database: LmdbDatabase) -> StoreResult<()>;
}

impl<T: WalletWriteTxn + ?Sized> WalletWriteTxnLmdbExt for T {
    fn put_lmdb(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: &[u8],
        flags: WriteFlags,
    ) -> StoreResult<()> {
        self.put(database.into(), key, value, from_lmdb_write_flags(flags))
    }

    fn delete_lmdb(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()> {
        self.delete(database.into(), key, value)
    }

    fn clear_db_lmdb(&mut self, database: LmdbDatabase) -> StoreResult<()> {
        self.clear_db(database.into())
    }

    fn open_rw_cursor_lmdb(&mut self, database: LmdbDatabase) -> StoreResult<LmdbRwCursor<'_>> {
        self.open_rw_cursor(database.into())
            .map(store_rw_cursor_into_lmdb)
    }

    unsafe fn drop_db_lmdb(&mut self, database: LmdbDatabase) -> StoreResult<()> {
        unsafe { self.drop_db(database.into()) }
    }
}

pub trait LedgerReadTxnLmdbExt {
    fn get_lmdb(&self, database: LmdbDatabase, key: &[u8]) -> StoreResult<&[u8]>;
    fn open_ro_cursor_lmdb(&self, database: LmdbDatabase) -> StoreResult<LmdbRoCursor<'_>>;
    fn count_lmdb(&self, database: LmdbDatabase) -> u64;
}

impl<T: LedgerReadTxn + ?Sized> LedgerReadTxnLmdbExt for T {
    fn get_lmdb(&self, database: LmdbDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        self.get(database.into(), key)
    }

    fn open_ro_cursor_lmdb(&self, database: LmdbDatabase) -> StoreResult<LmdbRoCursor<'_>> {
        self.open_ro_cursor(database.into())
            .map(store_ro_cursor_into_lmdb)
    }

    fn count_lmdb(&self, database: LmdbDatabase) -> u64 {
        self.count(database.into())
    }
}

pub trait LedgerWriteTxnLmdbExt: LedgerReadTxnLmdbExt {
    fn put_lmdb(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: &[u8],
        flags: WriteFlags,
    ) -> StoreResult<()>;

    fn delete_lmdb(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()>;

    fn clear_db_lmdb(&mut self, database: LmdbDatabase) -> StoreResult<()>;
    fn open_rw_cursor_lmdb(&mut self, database: LmdbDatabase) -> StoreResult<LmdbRwCursor<'_>>;
    unsafe fn drop_db_lmdb(&mut self, database: LmdbDatabase) -> StoreResult<()>;
}

impl<T: LedgerWriteTxn + ?Sized> LedgerWriteTxnLmdbExt for T {
    fn put_lmdb(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: &[u8],
        flags: WriteFlags,
    ) -> StoreResult<()> {
        self.put(database.into(), key, value, from_lmdb_write_flags(flags))
    }

    fn delete_lmdb(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()> {
        self.delete(database.into(), key, value)
    }

    fn clear_db_lmdb(&mut self, database: LmdbDatabase) -> StoreResult<()> {
        self.clear_db(database.into())
    }

    fn open_rw_cursor_lmdb(&mut self, database: LmdbDatabase) -> StoreResult<LmdbRwCursor<'_>> {
        self.open_rw_cursor(database.into())
            .map(store_rw_cursor_into_lmdb)
    }

    unsafe fn drop_db_lmdb(&mut self, database: LmdbDatabase) -> StoreResult<()> {
        unsafe { self.drop_db(database.into()) }
    }
}

impl WalletReadTxn for rsnano_nullable_lmdb::ReadTransaction {
    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        LmdbTxn::get(self, database.into(), key).map_err(Into::into)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        LmdbTxn::open_ro_cursor(self, database.into())
            .map(store_ro_cursor_from_lmdb)
            .map_err(Into::into)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        LmdbTxn::count(self, database.into())
    }

    fn commit(self: Box<Self>) {
        (*self).commit();
    }
}

impl WalletReadTxn for rsnano_nullable_lmdb::WriteTransaction {
    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        LmdbTxn::get(self, database.into(), key).map_err(Into::into)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        LmdbTxn::open_ro_cursor(self, database.into())
            .map(store_ro_cursor_from_lmdb)
            .map_err(Into::into)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        LmdbTxn::count(self, database.into())
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
        self.put(database.into(), key, value, to_lmdb_write_flags(flags))
            .map_err(StoreError::from)
    }

    fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()> {
        self.delete(database.into(), key, value)
            .map_err(StoreError::from)
    }

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        self.clear_db(database.into()).map_err(StoreError::from)
    }

    fn open_rw_cursor(&mut self, database: StoreDatabase) -> StoreResult<StoreRwCursor<'_>> {
        self.open_rw_cursor(database.into())
            .map(store_rw_cursor_from_lmdb)
            .map_err(StoreError::from)
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        unsafe { self.drop_db(database.into()) }.map_err(StoreError::from)
    }
}
