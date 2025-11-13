use std::sync::Arc;

use rsnano_nullable_lmdb::sys::{MDB_FIRST, MDB_NEXT};
use rsnano_nullable_lmdb::{
    EnvironmentOptions, LmdbEnvironment, LmdbEnvironmentFactory, ReadTransaction,
    Transaction as LmdbTxn, WriteTransaction,
};
use store_traits::environment::{
    StoreCursor, StoreEnvironment, StoreEnvironmentFactory, StoreEnvironmentOptions, StoreReadTxn,
    StoreWriteTxn,
};
use store_traits::types::{StoreDatabase, StoreError, StoreResult, StoreWriteFlags};

pub struct LmdbCursor<'txn> {
    inner: rsnano_nullable_lmdb::RoCursor<'txn>,
    started: bool,
}

impl<'txn> LmdbCursor<'txn> {
    pub fn new(inner: rsnano_nullable_lmdb::RoCursor<'txn>) -> Self {
        Self {
            inner,
            started: false,
        }
    }
}

impl<'txn> StoreCursor<'txn> for LmdbCursor<'txn> {
    fn next(&mut self) -> StoreResult<Option<(&'txn [u8], &'txn [u8])>> {
        let op = if self.started { MDB_NEXT } else { MDB_FIRST };
        self.started = true;
        match self.inner.get(None, None, op) {
            Ok((Some(key), value)) => Ok(Some((key, value))),
            Ok((None, _)) => Ok(None),
            Err(rsnano_nullable_lmdb::Error::NotFound) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

pub struct LmdbMutCursor<'txn> {
    inner: rsnano_nullable_lmdb::RwCursor<'txn>,
    started: bool,
}

impl<'txn> LmdbMutCursor<'txn> {
    pub fn new(inner: rsnano_nullable_lmdb::RwCursor<'txn>) -> Self {
        Self {
            inner,
            started: false,
        }
    }
}

impl<'txn> StoreCursor<'txn> for LmdbMutCursor<'txn> {
    fn next(&mut self) -> StoreResult<Option<(&'txn [u8], &'txn [u8])>> {
        let op = if self.started { MDB_NEXT } else { MDB_FIRST };
        self.started = true;
        match self.inner.get(None, None, op) {
            Ok((Some(key), value)) => Ok(Some((key, value))),
            Ok((None, _)) => Ok(None),
            Err(rsnano_nullable_lmdb::Error::NotFound) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

pub struct LmdbReadTxn<'env> {
    inner: ReadTransaction,
    _marker: std::marker::PhantomData<&'env ()>,
}

impl<'env> LmdbReadTxn<'env> {
    pub fn new(inner: ReadTransaction) -> Self {
        Self {
            inner,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<'env> StoreReadTxn<'env> for LmdbReadTxn<'env> {
    type Cursor<'txn>
        = LmdbCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

    fn get<'txn>(&'txn self, database: StoreDatabase, key: &[u8]) -> StoreResult<&'txn [u8]>
    where
        'env: 'txn,
    {
        LmdbTxn::get(&self.inner, database.into(), key).map_err(Into::into)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        LmdbTxn::count(&self.inner, database.into())
    }

    fn open_cursor<'txn>(&'txn self, database: StoreDatabase) -> StoreResult<Self::Cursor<'txn>>
    where
        'env: 'txn,
    {
        let cursor =
            LmdbTxn::open_ro_cursor(&self.inner, database.into()).map_err(StoreError::from)?;
        Ok(LmdbCursor::new(cursor))
    }

    fn commit(self)
    where
        Self: Sized,
    {
        self.inner.commit();
    }
}

pub struct LmdbWriteTxn<'env> {
    inner: WriteTransaction,
    _marker: std::marker::PhantomData<&'env ()>,
}

impl<'env> LmdbWriteTxn<'env> {
    pub fn new(inner: WriteTransaction) -> Self {
        Self {
            inner,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<'env> StoreReadTxn<'env> for LmdbWriteTxn<'env> {
    type Cursor<'txn>
        = LmdbCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

    fn get<'txn>(&'txn self, database: StoreDatabase, key: &[u8]) -> StoreResult<&'txn [u8]>
    where
        'env: 'txn,
    {
        LmdbTxn::get(&self.inner, database.into(), key).map_err(Into::into)
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        LmdbTxn::count(&self.inner, database.into())
    }

    fn open_cursor<'txn>(&'txn self, database: StoreDatabase) -> StoreResult<Self::Cursor<'txn>>
    where
        'env: 'txn,
    {
        let cursor =
            LmdbTxn::open_ro_cursor(&self.inner, database.into()).map_err(StoreError::from)?;
        Ok(LmdbCursor::new(cursor))
    }

    fn commit(self)
    where
        Self: Sized,
    {
        self.inner.commit();
    }
}

impl<'env> StoreWriteTxn<'env> for LmdbWriteTxn<'env> {
    type MutCursor<'txn>
        = LmdbMutCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

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

    fn open_rw_cursor<'txn>(
        &'txn mut self,
        database: StoreDatabase,
    ) -> StoreResult<Self::MutCursor<'txn>>
    where
        'env: 'txn,
    {
        self.inner
            .open_rw_cursor(database.into())
            .map(LmdbMutCursor::new)
            .map_err(Into::into)
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        unsafe { self.inner.drop_db(database.into()) }.map_err(Into::into)
    }
}

pub struct LmdbStoreEnvironment {
    inner: Arc<LmdbEnvironment>,
}

impl LmdbStoreEnvironment {
    pub fn new(inner: LmdbEnvironment) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn from_arc(inner: Arc<LmdbEnvironment>) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> &LmdbEnvironment {
        &self.inner
    }
}

impl StoreEnvironment for LmdbStoreEnvironment {
    type ReadTxn<'env>
        = LmdbReadTxn<'env>
    where
        Self: 'env;
    type WriteTxn<'env>
        = LmdbWriteTxn<'env>
    where
        Self: 'env;

    fn begin_read(&self) -> Self::ReadTxn<'_> {
        LmdbReadTxn::new(self.inner.begin_read())
    }

    fn begin_write(&self) -> Self::WriteTxn<'_> {
        LmdbWriteTxn::new(self.inner.begin_write())
    }

    fn open_db(&self, name: Option<&str>) -> StoreResult<StoreDatabase> {
        self.inner
            .open_db(name)
            .map(StoreDatabase::from)
            .map_err(Into::into)
    }

    fn sync(&self) -> StoreResult<()> {
        self.inner.sync().map_err(Into::into)
    }
}

pub struct LmdbStoreEnvironmentFactory {
    inner: LmdbEnvironmentFactory,
}

impl Default for LmdbStoreEnvironmentFactory {
    fn default() -> Self {
        Self {
            inner: LmdbEnvironmentFactory::default(),
        }
    }
}

impl LmdbStoreEnvironmentFactory {
    pub fn new(inner: LmdbEnvironmentFactory) -> Self {
        Self { inner }
    }

    fn to_lmdb_options(options: StoreEnvironmentOptions) -> EnvironmentOptions {
        EnvironmentOptions {
            max_dbs: options.max_databases,
            map_size: options.map_size,
            flags: options.flags.into(),
            path: options.path,
        }
    }
}

impl StoreEnvironmentFactory for LmdbStoreEnvironmentFactory {
    type Environment = LmdbStoreEnvironment;

    fn create(&self, options: StoreEnvironmentOptions) -> anyhow::Result<Arc<Self::Environment>> {
        let env = self.inner.create(Self::to_lmdb_options(options))?;
        Ok(Arc::new(LmdbStoreEnvironment::new(env)))
    }

    fn create_null(&self) -> Arc<Self::Environment> {
        Arc::new(LmdbStoreEnvironment::new(LmdbEnvironment::new_null()))
    }
}
