use std::{path::PathBuf, sync::Arc};

use crate::types::{StoreDatabase, StoreEnvironmentFlags, StoreResult, StoreWriteFlags};

/// Cursor over a database view that yields key/value pairs tied to the cursor lifetime.
pub trait StoreCursor<'txn> {
    fn next(&mut self) -> StoreResult<Option<(&'txn [u8], &'txn [u8])>>;
}

/// Read-only transaction interface with lifetime-aware cursors.
pub trait StoreReadTxn<'env>: 'env {
    type Cursor<'txn>: StoreCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

    fn get<'txn>(&'txn self, database: StoreDatabase, key: &[u8]) -> StoreResult<&'txn [u8]>
    where
        'env: 'txn;

    fn count(&self, database: StoreDatabase) -> StoreResult<u64>;

    fn open_cursor<'txn>(&'txn self, database: StoreDatabase) -> StoreResult<Self::Cursor<'txn>>
    where
        'env: 'txn;

    fn commit(self) -> StoreResult<()>
    where
        Self: Sized;
}

/// Write transaction with mutation capabilities layered on top of read access.
pub trait StoreWriteTxn<'env>: StoreReadTxn<'env> {
    type MutCursor<'txn>: StoreCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

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

    fn open_rw_cursor<'txn>(
        &'txn mut self,
        database: StoreDatabase,
    ) -> StoreResult<Self::MutCursor<'txn>>
    where
        'env: 'txn;

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()>;
}

/// Backend-agnostic database environment.
pub trait StoreEnvironment: Send + Sync {
    type ReadTxn<'env>: StoreReadTxn<'env>
    where
        Self: 'env;
    type WriteTxn<'env>: StoreWriteTxn<'env>
    where
        Self: 'env;

    fn begin_read(&self) -> Self::ReadTxn<'_>;
    fn begin_write(&self) -> Self::WriteTxn<'_>;
    fn open_db(&self, name: Option<&str>) -> StoreResult<StoreDatabase>;
    fn sync(&self) -> StoreResult<()>;
}

/// Factory for constructing store environments.
pub trait StoreEnvironmentFactory: Send + Sync {
    type Environment: StoreEnvironment;

    fn create(&self, options: StoreEnvironmentOptions) -> anyhow::Result<Arc<Self::Environment>>;

    fn create_null(&self) -> Arc<Self::Environment>;
}

#[derive(Clone)]
pub struct StoreEnvironmentOptions {
    pub path: PathBuf,
    pub max_databases: u32,
    pub map_size: usize,
    pub flags: StoreEnvironmentFlags,
}
