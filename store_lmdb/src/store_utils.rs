use rsnano_nullable_lmdb::{
    EnvironmentFlags, Error as LmdbError, LmdbDatabase, RoCursor as LmdbRoCursor,
    RwCursor as LmdbRwCursor, WriteFlags,
};
use std::num::NonZeroUsize;
use store_traits::types::{
    StoreDatabase, StoreEnvironmentFlags, StoreError, StoreErrorKind, StoreRoCursor, StoreRwCursor,
    StoreWriteFlags,
};

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

const HANDLE_STUB_FLAG: usize = 1;

pub(crate) fn store_database_from_lmdb(database: LmdbDatabase) -> StoreDatabase {
    let (is_stub, id) = database.to_raw_parts();
    let encoded = encode_handle(id, is_stub);
    unsafe { StoreDatabase::from_raw(encoded) }
}

pub(crate) fn lmdb_database_from_store(database: StoreDatabase) -> LmdbDatabase {
    let (id, is_stub) = decode_handle(database.into_raw());
    if is_stub {
        LmdbDatabase::from_raw_parts(true, id)
    } else {
        LmdbDatabase::from_raw_parts(false, id)
    }
}

pub(crate) fn store_error_from_lmdb(error: LmdbError) -> StoreError {
    match error {
        LmdbError::NotFound => StoreError::not_found(),
        LmdbError::MapFull => {
            StoreError::new(StoreErrorKind::MapFull, error.to_string().into_boxed_str())
        }
        LmdbError::Invalid => StoreError::new(
            StoreErrorKind::InvalidArgument,
            error.to_string().into_boxed_str(),
        ),
        LmdbError::Corrupted => StoreError::new(
            StoreErrorKind::Corruption,
            error.to_string().into_boxed_str(),
        ),
        LmdbError::PageNotFound => StoreError::new(
            StoreErrorKind::PageNotFound,
            error.to_string().into_boxed_str(),
        ),
        _ => StoreError::backend(error.to_string()),
    }
}

fn encode_handle(id: u32, is_stub: bool) -> NonZeroUsize {
    let mut raw = ((id as usize) << 1) | if is_stub { HANDLE_STUB_FLAG } else { 0 };
    raw += 1;
    NonZeroUsize::new(raw).expect("raw handles are always incremented")
}

fn decode_handle(raw: NonZeroUsize) -> (u32, bool) {
    let mut value = raw.get() - 1;
    let is_stub = (value & HANDLE_STUB_FLAG) == HANDLE_STUB_FLAG;
    value >>= 1;
    (value as u32, is_stub)
}

unsafe fn drop_lmdb_ro_cursor(handle: NonZeroUsize) {
    let ptr = handle.get() as *mut LmdbRoCursor<'static>;
    drop(unsafe { Box::from_raw(ptr) });
}

unsafe fn drop_lmdb_rw_cursor(handle: NonZeroUsize) {
    let ptr = handle.get() as *mut LmdbRwCursor<'static>;
    drop(unsafe { Box::from_raw(ptr) });
}
