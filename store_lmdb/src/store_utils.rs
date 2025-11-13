use std::num::NonZeroUsize;

use rsnano_nullable_lmdb::{
    EnvironmentFlags, RoCursor as LmdbRoCursor, RwCursor as LmdbRwCursor, WriteFlags,
};
use store_traits::types::{StoreEnvironmentFlags, StoreRoCursor, StoreRwCursor, StoreWriteFlags};

pub(crate) fn store_ro_cursor_from_lmdb<'txn>(cursor: LmdbRoCursor<'txn>) -> StoreRoCursor<'txn> {
    let raw = Box::into_raw(Box::new(cursor)) as usize;
    let handle = unsafe { NonZeroUsize::new_unchecked(raw) };
    unsafe { StoreRoCursor::from_raw_parts(handle, drop_lmdb_ro_cursor) }
}

pub(crate) fn store_rw_cursor_from_lmdb<'txn>(cursor: LmdbRwCursor<'txn>) -> StoreRwCursor<'txn> {
    let raw = Box::into_raw(Box::new(cursor)) as usize;
    let handle = unsafe { NonZeroUsize::new_unchecked(raw) };
    unsafe { StoreRwCursor::from_raw_parts(handle, drop_lmdb_rw_cursor) }
}

pub(crate) fn lmdb_ro_cursor_from_store<'txn>(cursor: StoreRoCursor<'txn>) -> LmdbRoCursor<'txn> {
    let (handle, _) = cursor.into_raw_parts();
    let ptr = handle.get() as *mut LmdbRoCursor<'txn>;
    *unsafe { Box::from_raw(ptr) }
}

pub(crate) fn store_write_flags_from(flags: WriteFlags) -> StoreWriteFlags {
    StoreWriteFlags::from_bits(flags.bits())
}

pub(crate) fn lmdb_write_flags_from(flags: StoreWriteFlags) -> WriteFlags {
    WriteFlags::from_bits_truncate(flags.bits())
}

pub(crate) fn lmdb_env_flags_from(flags: StoreEnvironmentFlags) -> EnvironmentFlags {
    EnvironmentFlags::from_bits_truncate(flags.bits())
}

unsafe fn drop_lmdb_ro_cursor(handle: NonZeroUsize) {
    let ptr = handle.get() as *mut LmdbRoCursor<'static>;
    drop(unsafe { Box::from_raw(ptr) });
}

unsafe fn drop_lmdb_rw_cursor(handle: NonZeroUsize) {
    let ptr = handle.get() as *mut LmdbRwCursor<'static>;
    drop(unsafe { Box::from_raw(ptr) });
}
