use std::{num::NonZeroUsize, sync::Arc};

use rocksdb::{DBRawIteratorWithThreadMode, ReadOptions, WriteBatchWithIndex};
use store_traits::environment::{StoreCursor, StoreReadTxn, StoreWriteTxn};
use store_traits::transaction::{LedgerReadTxn, LedgerWriteTxn};
use store_traits::types::{
    StoreDatabase, StoreError, StoreResult, StoreRoCursor, StoreRwCursor, StoreValue,
    StoreWriteFlags,
};

use crate::environment::{
    RocksDb, RocksDbInner, RocksDbSnapshot, RocksdbStoreEnvironment, store_error_from_rocksdb,
};

pub struct RocksdbLedgerReadTxn {
    inner: RocksdbReadTxn<'static>,
}

impl RocksdbLedgerReadTxn {
    pub fn new(env: &Arc<RocksdbStoreEnvironment>) -> Self {
        let inner = env.inner();
        let txn = RocksdbReadTxn::new(&inner);
        let txn_static: RocksdbReadTxn<'static> = unsafe { std::mem::transmute(txn) };
        Self { inner: txn_static }
    }
}

pub struct RocksdbLedgerWriteTxn {
    inner: RocksdbWriteTxn<'static>,
}

impl RocksdbLedgerWriteTxn {
    pub fn new(env: &Arc<RocksdbStoreEnvironment>) -> Self {
        let inner = env.inner();
        let txn = RocksdbWriteTxn::new(&inner);
        let txn_static: RocksdbWriteTxn<'static> = unsafe { std::mem::transmute(txn) };
        Self { inner: txn_static }
    }

    pub fn as_inner_mut(&mut self) -> &mut RocksdbWriteTxn<'static> {
        &mut self.inner
    }
}

impl LedgerReadTxn for RocksdbLedgerReadTxn {
    fn is_refresh_needed(&self) -> bool {
        false
    }

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<StoreValue> {
        self.inner.get(database, key)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        let cursor = self.inner.open_cursor(database)?;
        Ok(store_ro_cursor_from_rocksdb(cursor))
    }

    fn count(&self, database: StoreDatabase) -> StoreResult<u64> {
        self.inner.count(database)
    }
}

impl LedgerReadTxn for RocksdbLedgerWriteTxn {
    fn is_refresh_needed(&self) -> bool {
        false
    }

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<StoreValue> {
        self.inner.get(database, key)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        let cursor = self.inner.open_cursor(database)?;
        Ok(store_ro_cursor_from_rocksdb(cursor))
    }

    fn count(&self, database: StoreDatabase) -> StoreResult<u64> {
        self.inner.count(database)
    }
}

impl LedgerWriteTxn for RocksdbLedgerWriteTxn {
    fn put(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: &[u8],
        flags: StoreWriteFlags,
    ) -> StoreResult<()> {
        self.inner.put(database, key, value, flags)
    }

    fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()> {
        self.inner.delete(database, key, value)
    }

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        self.inner.clear_db(database)
    }

    fn open_rw_cursor(&mut self, database: StoreDatabase) -> StoreResult<StoreRwCursor<'_>> {
        let cursor = self.inner.open_rw_cursor(database)?;
        Ok(store_rw_cursor_from_rocksdb(cursor))
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        unsafe { self.inner.drop_db(database) }
    }

    fn commit(self: Box<Self>) -> StoreResult<()> {
        self.inner.commit()
    }
}

pub struct RocksdbReadTxn<'env> {
    inner: Arc<RocksDbInner>,
    snapshot: RocksDbSnapshot<'env>,
}

impl<'env> RocksdbReadTxn<'env> {
    pub(crate) fn new(inner: &'env Arc<RocksDbInner>) -> Self {
        let snapshot = inner.snapshot();
        Self {
            inner: Arc::clone(inner),
            snapshot,
        }
    }
}

impl<'env> StoreReadTxn<'env> for RocksdbReadTxn<'env> {
    type Cursor<'txn>
        = RocksdbCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<StoreValue> {
        let handle = self.inner.cf_handle(database)?;
        match self
            .snapshot
            .get_pinned_cf(&handle, key)
            .map_err(store_error_from_rocksdb)?
        {
            Some(value) => Ok(StoreValue::from_slice(value.as_ref())),
            None => Err(StoreError::not_found()),
        }
    }

    fn count(&self, database: StoreDatabase) -> StoreResult<u64> {
        self.inner.count_snapshot_entries(&self.snapshot, database)
    }

    fn open_cursor<'txn>(&'txn self, database: StoreDatabase) -> StoreResult<Self::Cursor<'txn>>
    where
        'env: 'txn,
    {
        let handle = self.inner.cf_handle(database)?;
        let iter = self.snapshot.raw_iterator_cf(&handle);
        Ok(RocksdbCursor::from_raw_iterator(iter))
    }

    fn commit(self) -> StoreResult<()>
    where
        Self: Sized,
    {
        Ok(())
    }
}

pub struct RocksdbWriteTxn<'env> {
    inner: Arc<RocksDbInner>,
    snapshot: RocksDbSnapshot<'env>,
    batch: WriteBatchWithIndex,
}

impl<'env> RocksdbWriteTxn<'env> {
    pub(crate) fn new(inner: &'env Arc<RocksDbInner>) -> Self {
        let snapshot = inner.snapshot();
        Self {
            inner: Arc::clone(inner),
            snapshot,
            batch: WriteBatchWithIndex::new(0, true),
        }
    }

    fn snapshot_read_options(&self) -> ReadOptions {
        let mut read_options = ReadOptions::default();
        read_options.set_snapshot(&self.snapshot);
        read_options
    }

    fn batch_raw_iterator<'txn>(
        &'txn self,
        database: StoreDatabase,
    ) -> StoreResult<DBRawIteratorWithThreadMode<'txn, RocksDb>>
    where
        'env: 'txn,
    {
        let handle = self.inner.cf_handle(database)?;
        let read_options = self.snapshot_read_options();
        let base = self.snapshot.raw_iterator_cf_opt(&handle, read_options);
        Ok(self.batch.iterator_with_base_cf(base, &handle))
    }
}

impl<'env> StoreReadTxn<'env> for RocksdbWriteTxn<'env> {
    type Cursor<'txn>
        = RocksdbCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<StoreValue> {
        let handle = self.inner.cf_handle(database)?;
        let read_options = self.snapshot_read_options();
        match self
            .batch
            .get_from_batch_and_db_cf(&self.inner.db, &handle, key, &read_options)
            .map_err(store_error_from_rocksdb)?
        {
            Some(value) => Ok(StoreValue::from(value)),
            None => Err(StoreError::not_found()),
        }
    }

    fn count(&self, database: StoreDatabase) -> StoreResult<u64> {
        let mut cursor = self.open_cursor(database)?;
        let mut count = 0u64;
        while cursor.next()?.is_some() {
            count += 1;
        }
        Ok(count)
    }

    fn open_cursor<'txn>(&'txn self, database: StoreDatabase) -> StoreResult<Self::Cursor<'txn>>
    where
        'env: 'txn,
    {
        let iter = self.batch_raw_iterator(database)?;
        Ok(RocksdbCursor::from_raw_iterator(iter))
    }

    fn commit(self) -> StoreResult<()>
    where
        Self: Sized,
    {
        self.inner
            .db
            .write_wbwi(&self.batch)
            .map_err(store_error_from_rocksdb)
    }
}

impl<'env> StoreWriteTxn<'env> for RocksdbWriteTxn<'env> {
    type MutCursor<'txn>
        = RocksdbCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

    fn put(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: &[u8],
        _flags: StoreWriteFlags,
    ) -> StoreResult<()> {
        let handle = self.inner.cf_handle(database)?;
        self.batch.put_cf(&handle, key, value);
        Ok(())
    }

    fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        _value: Option<&[u8]>,
    ) -> StoreResult<()> {
        let handle = self.inner.cf_handle(database)?;
        self.batch.delete_cf(&handle, key);
        Ok(())
    }

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        let keys = {
            let mut cursor = self.open_cursor(database)?;
            let mut removed = Vec::new();
            while let Some((key, _)) = cursor.next()? {
                removed.push(key.as_ref().to_vec());
            }
            removed
        };
        let handle = self.inner.cf_handle(database)?;
        for key in keys {
            self.batch.delete_cf(&handle, &key);
        }
        Ok(())
    }

    fn open_rw_cursor<'txn>(
        &'txn mut self,
        database: StoreDatabase,
    ) -> StoreResult<Self::MutCursor<'txn>>
    where
        'env: 'txn,
    {
        let iter = self.batch_raw_iterator(database)?;
        Ok(RocksdbCursor::from_raw_iterator(iter))
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        self.inner.delete_cf(database)
    }
}

pub struct RocksdbCursor<'txn> {
    iter: DBRawIteratorWithThreadMode<'txn, RocksDb>,
    started: bool,
}

impl<'txn> RocksdbCursor<'txn> {
    fn from_raw_iterator(iter: DBRawIteratorWithThreadMode<'txn, RocksDb>) -> Self {
        Self {
            iter,
            started: false,
        }
    }

    fn advance(&mut self) {
        if !self.started {
            self.iter.seek_to_first();
            self.started = true;
        } else {
            self.iter.next();
        }
    }
}

impl<'txn> StoreCursor<'txn> for RocksdbCursor<'txn> {
    fn next(&mut self) -> StoreResult<Option<(StoreValue, StoreValue)>> {
        self.advance();
        if !self.iter.valid() {
            self.iter.status().map_err(store_error_from_rocksdb)?;
            return Ok(None);
        }
        let key = self.iter.key().expect("iterator valid without key");
        let value = self.iter.value().expect("iterator valid without value");
        Ok(Some((
            StoreValue::from_slice(key),
            StoreValue::from_slice(value),
        )))
    }
}

pub(crate) fn store_ro_cursor_from_rocksdb<'txn>(
    cursor: RocksdbCursor<'txn>,
) -> StoreRoCursor<'txn> {
    let boxed = Box::new(cursor);
    let ptr = Box::into_raw(boxed);
    let handle = NonZeroUsize::new(ptr as usize).expect("non-zero cursor pointer");
    unsafe { StoreRoCursor::from_raw_parts(handle, drop_rocksdb_ro_cursor) }
}

pub(crate) fn store_rw_cursor_from_rocksdb<'txn>(
    cursor: RocksdbCursor<'txn>,
) -> StoreRwCursor<'txn> {
    let boxed = Box::new(cursor);
    let ptr = Box::into_raw(boxed);
    let handle = NonZeroUsize::new(ptr as usize).expect("non-zero cursor pointer");
    unsafe { StoreRwCursor::from_raw_parts(handle, drop_rocksdb_rw_cursor) }
}

unsafe fn drop_rocksdb_ro_cursor(handle: NonZeroUsize) {
    let ptr = handle.get() as *mut RocksdbCursor<'static>;
    unsafe {
        drop(Box::from_raw(ptr));
    }
}

unsafe fn drop_rocksdb_rw_cursor(handle: NonZeroUsize) {
    let ptr = handle.get() as *mut RocksdbCursor<'static>;
    unsafe {
        drop(Box::from_raw(ptr));
    }
}

pub(crate) fn rocksdb_ro_cursor_from_store<'txn>(
    cursor: StoreRoCursor<'txn>,
) -> RocksdbCursor<'txn> {
    let handle = cursor.into_raw_parts().0;
    let ptr = handle.get() as *mut RocksdbCursor<'txn>;
    unsafe { *Box::from_raw(ptr) }
}
