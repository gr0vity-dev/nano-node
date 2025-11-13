pub type StoreDatabase = rsnano_nullable_lmdb::LmdbDatabase;
pub type StoreResult<T> = rsnano_nullable_lmdb::Result<T>;
pub type StoreError = rsnano_nullable_lmdb::Error;
pub type StoreRoCursor<'txn> = rsnano_nullable_lmdb::RoCursor<'txn>;
pub type StoreRwCursor<'txn> = rsnano_nullable_lmdb::RwCursor<'txn>;
pub type StoreWriteFlags = rsnano_nullable_lmdb::WriteFlags;
pub type StoreWriteTransaction = rsnano_nullable_lmdb::WriteTransaction;
pub use rsnano_nullable_lmdb::Transaction as StoreBackendTransaction;
