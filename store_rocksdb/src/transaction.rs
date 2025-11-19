use rocksdb::{
    BoundColumnFamily, DBRawIteratorWithThreadMode, OptimisticTransactionOptions, ReadOptions,
    Transaction, WriteOptions,
};
use std::{num::NonZeroUsize, sync::Arc};
use store_traits::environment::{StoreCursor, StoreReadTxn, StoreWriteTxn};
use store_traits::transaction::{LedgerReadTxn, LedgerWriteTxn};
use store_traits::types::{
    StoreDatabase, StoreError, StoreErrorKind, StoreResult, StoreRoCursor, StoreRwCursor,
    StoreValue, StoreWriteFlags,
};

use crate::environment::{
    RocksDb, RocksDbInner, RocksDbSnapshot, RocksdbStoreEnvironment, store_error_from_rocksdb,
};

struct SnapshotResources {
    snapshot: RocksDbSnapshot<'static>,
    inner: Arc<RocksDbInner>,
}

impl SnapshotResources {
    fn new(inner: &Arc<RocksDbInner>) -> Arc<Self> {
        let owned_inner = Arc::clone(inner);
        let raw = Arc::into_raw(Arc::clone(inner));
        let static_ref: &'static RocksDbInner = unsafe { &*raw };
        let snapshot = static_ref.snapshot();
        unsafe {
            Arc::from_raw(raw);
        }
        Arc::new(Self {
            snapshot,
            inner: owned_inner,
        })
    }

    fn cf_handle(&self, database: StoreDatabase) -> StoreResult<Arc<BoundColumnFamily<'_>>> {
        self.inner.cf_handle(database)
    }
}

struct SnapshotView {
    resources: Arc<SnapshotResources>,
}

impl SnapshotView {
    fn new(inner: &Arc<RocksDbInner>) -> Self {
        Self {
            resources: SnapshotResources::new(inner),
        }
    }

    fn resources(&self) -> &Arc<SnapshotResources> {
        &self.resources
    }

    fn inner(&self) -> &Arc<RocksDbInner> {
        &self.resources.inner
    }

    fn snapshot(&self) -> &RocksDbSnapshot<'static> {
        &self.resources.snapshot
    }
}

pub struct RocksdbLedgerReadTxn {
    inner: RocksdbReadTxn,
}

impl RocksdbLedgerReadTxn {
    pub fn new(env: &Arc<RocksdbStoreEnvironment>) -> Self {
        let inner = env.inner();
        let txn = RocksdbReadTxn::new(&inner);
        Self { inner: txn }
    }
}

pub struct RocksdbLedgerWriteTxn {
    inner: RocksdbWriteTxn,
}

impl RocksdbLedgerWriteTxn {
    pub fn new(env: &Arc<RocksdbStoreEnvironment>) -> Self {
        let inner = env.inner();
        let txn = RocksdbWriteTxn::new(&inner);
        Self { inner: txn }
    }

    pub fn as_inner_mut(&mut self) -> &mut RocksdbWriteTxn {
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

pub struct RocksdbReadTxn {
    view: SnapshotView,
}

impl RocksdbReadTxn {
    pub(crate) fn new(inner: &Arc<RocksDbInner>) -> Self {
        let view = SnapshotView::new(inner);
        Self { view }
    }
}

impl<'env> StoreReadTxn<'env> for RocksdbReadTxn {
    type Cursor<'txn>
        = RocksdbCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<StoreValue> {
        let handle = self.view.resources().cf_handle(database)?;
        let read_options = ReadOptions::default();
        match self
            .view
            .snapshot()
            .get_cf_opt(&handle, key, read_options)
            .map_err(store_error_from_rocksdb)?
        {
            Some(value) => Ok(StoreValue::from(value)),
            None => Err(StoreError::not_found()),
        }
    }

    fn count(&self, database: StoreDatabase) -> StoreResult<u64> {
        self.view
            .inner()
            .count_snapshot_entries(self.view.snapshot(), database)
    }

    fn open_cursor<'txn>(&'txn self, database: StoreDatabase) -> StoreResult<Self::Cursor<'txn>>
    where
        'env: 'txn,
    {
        let handle = self.view.inner().cf_handle(database)?;
        let iter = self.view.snapshot().raw_iterator_cf(&handle);
        Ok(RocksdbCursor::from_snapshot_iter(iter))
    }

    fn commit(self) -> StoreResult<()>
    where
        Self: Sized,
    {
        Ok(())
    }
}

pub struct RocksdbWriteTxn {
    inner: Arc<RocksDbInner>,
    txn: Transaction<'static, RocksDb>,
}

impl RocksdbWriteTxn {
    pub(crate) fn new(inner: &Arc<RocksDbInner>) -> Self {
        let raw = Arc::into_raw(Arc::clone(inner));
        let static_inner: &'static RocksDbInner = unsafe { &*raw };
        let write_opts = WriteOptions::default();
        let mut txn_opts = OptimisticTransactionOptions::new();
        txn_opts.set_snapshot(true);
        let txn = static_inner.db.transaction_opt(&write_opts, &txn_opts);
        unsafe {
            Arc::from_raw(raw);
        }
        Self {
            inner: Arc::clone(inner),
            txn,
        }
    }

    fn map_txn_error(err: rocksdb::Error) -> StoreError {
        match err.kind() {
            rocksdb::ErrorKind::Busy | rocksdb::ErrorKind::TryAgain => {
                StoreError::new(StoreErrorKind::Conflict, err.into_string())
            }
            _ => store_error_from_rocksdb(err),
        }
    }
}

impl<'env> StoreReadTxn<'env> for RocksdbWriteTxn {
    type Cursor<'txn>
        = RocksdbCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<StoreValue> {
        let handle = self.inner.cf_handle(database)?;
        match self
            .txn
            .get_cf(&handle, key)
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
        let handle = self.inner.cf_handle(database)?;
        let iter = self.txn.raw_iterator_cf(&handle);
        Ok(RocksdbCursor::from_txn_iter(iter))
    }

    fn commit(self) -> StoreResult<()>
    where
        Self: Sized,
    {
        self.txn.commit().map_err(Self::map_txn_error)
    }
}

impl<'env> StoreWriteTxn<'env> for RocksdbWriteTxn {
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
        self.txn
            .put_cf(&handle, key, value)
            .map_err(store_error_from_rocksdb)
    }

    fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        _value: Option<&[u8]>,
    ) -> StoreResult<()> {
        let handle = self.inner.cf_handle(database)?;
        self.txn
            .delete_cf(&handle, key)
            .map_err(store_error_from_rocksdb)
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
            self.txn
                .delete_cf(&handle, &key)
                .map_err(store_error_from_rocksdb)?;
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
        let handle = self.inner.cf_handle(database)?;
        let iter = self.txn.raw_iterator_cf(&handle);
        Ok(RocksdbCursor::from_txn_iter(iter))
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        self.inner.delete_cf(database)
    }
}

enum RocksdbCursorIter<'txn> {
    Db(DBRawIteratorWithThreadMode<'txn, RocksDb>),
    Txn(DBRawIteratorWithThreadMode<'txn, Transaction<'static, RocksDb>>),
}

pub struct RocksdbCursor<'txn> {
    iter: RocksdbCursorIter<'txn>,
    started: bool,
}

impl<'txn> RocksdbCursor<'txn> {
    fn from_snapshot_iter(iter: DBRawIteratorWithThreadMode<'txn, RocksDb>) -> Self {
        Self {
            iter: RocksdbCursorIter::Db(iter),
            started: false,
        }
    }

    fn from_txn_iter(
        iter: DBRawIteratorWithThreadMode<'txn, Transaction<'static, RocksDb>>,
    ) -> Self {
        Self {
            iter: RocksdbCursorIter::Txn(iter),
            started: false,
        }
    }

    fn advance(&mut self) {
        match &mut self.iter {
            RocksdbCursorIter::Db(iter) => {
                if !self.started {
                    iter.seek_to_first();
                    self.started = true;
                } else {
                    iter.next();
                }
            }
            RocksdbCursorIter::Txn(iter) => {
                if !self.started {
                    iter.seek_to_first();
                    self.started = true;
                } else {
                    iter.next();
                }
            }
        }
    }

    fn current_entry(&self) -> StoreResult<Option<(StoreValue, StoreValue)>> {
        match &self.iter {
            RocksdbCursorIter::Db(iter) => {
                if !iter.valid() {
                    iter.status().map_err(store_error_from_rocksdb)?;
                    return Ok(None);
                }
                let key = iter.key().expect("iterator valid without key");
                let value = iter.value().expect("iterator valid without value");
                Ok(Some((
                    StoreValue::from_slice(key),
                    StoreValue::from_slice(value),
                )))
            }
            RocksdbCursorIter::Txn(iter) => {
                if !iter.valid() {
                    iter.status().map_err(store_error_from_rocksdb)?;
                    return Ok(None);
                }
                let key = iter.key().expect("iterator valid without key");
                let value = iter.value().expect("iterator valid without value");
                Ok(Some((
                    StoreValue::from_slice(key),
                    StoreValue::from_slice(value),
                )))
            }
        }
    }
}

impl<'txn> StoreCursor<'txn> for RocksdbCursor<'txn> {
    fn next(&mut self) -> StoreResult<Option<(StoreValue, StoreValue)>> {
        self.advance();
        self.current_entry()
    }

    fn seek_lower_bound(&mut self, key: &[u8]) -> StoreResult<Option<(StoreValue, StoreValue)>> {
        match &mut self.iter {
            RocksdbCursorIter::Db(iter) => iter.seek(key),
            RocksdbCursorIter::Txn(iter) => iter.seek(key),
        }
        self.started = true;
        self.current_entry()
    }

    fn seek_upper_bound(&mut self, key: &[u8]) -> StoreResult<Option<(StoreValue, StoreValue)>> {
        match &mut self.iter {
            RocksdbCursorIter::Db(iter) => {
                iter.seek(key);
                self.started = true;
                if iter.valid() {
                    if let Some(current_key) = iter.key() {
                        if current_key == key {
                            iter.next();
                        }
                    }
                }
            }
            RocksdbCursorIter::Txn(iter) => {
                iter.seek(key);
                self.started = true;
                if iter.valid() {
                    if let Some(current_key) = iter.key() {
                        if current_key == key {
                            iter.next();
                        }
                    }
                }
            }
        }
        self.current_entry()
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
