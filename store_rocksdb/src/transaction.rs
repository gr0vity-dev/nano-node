use std::{cell::RefCell, collections::BTreeMap, num::NonZeroUsize, sync::Arc};

use rocksdb::{IteratorMode, WriteBatch};
use store_traits::environment::{StoreCursor, StoreReadTxn, StoreWriteTxn};
use store_traits::transaction::{LedgerReadTxn, LedgerWriteTxn};
use store_traits::types::{
    StoreDatabase, StoreError, StoreResult, StoreRoCursor, StoreRwCursor, StoreWriteFlags,
};

use crate::environment::{
    RocksDbInner, RocksDbSnapshot, RocksdbStoreEnvironment, store_error_from_rocksdb,
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

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
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

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
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
    buffers: RefCell<Vec<Vec<u8>>>,
}

impl<'env> RocksdbReadTxn<'env> {
    pub(crate) fn new(inner: &'env Arc<RocksDbInner>) -> Self {
        let snapshot = inner.snapshot();
        Self {
            inner: Arc::clone(inner),
            snapshot,
            buffers: RefCell::new(Vec::new()),
        }
    }

    fn cache_bytes<'txn>(&'txn self, data: &[u8]) -> &'txn [u8]
    where
        'env: 'txn,
    {
        let mut buffers = self.buffers.borrow_mut();
        buffers.push(data.to_vec());
        let idx = buffers.len() - 1;
        let ptr: *const Vec<u8> = &buffers[idx];
        drop(buffers);
        unsafe { (&*ptr).as_slice() }
    }

    fn cursor_cache<'txn>(&'txn self) -> CursorCache<'txn>
    where
        'env: 'txn,
    {
        CursorCache::new(&self.buffers)
    }
}

impl<'env> StoreReadTxn<'env> for RocksdbReadTxn<'env> {
    type Cursor<'txn>
        = RocksdbCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

    fn get<'txn>(&'txn self, database: StoreDatabase, key: &[u8]) -> StoreResult<&'txn [u8]>
    where
        'env: 'txn,
    {
        let handle = self.inner.cf_handle(database)?;
        match self
            .snapshot
            .get_pinned_cf(&handle, key)
            .map_err(store_error_from_rocksdb)?
        {
            Some(value) => Ok(self.cache_bytes(value.as_ref())),
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
        let map = self.inner.snapshot_entries_map(&self.snapshot, database)?;
        let entries = map
            .into_iter()
            .map(|(k, v)| (k.into_boxed_slice(), v.into_boxed_slice()))
            .collect();
        Ok(RocksdbCursor::from_entries(self.cursor_cache(), entries))
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
    batch: WriteBatch,
    buffers: RefCell<Vec<Vec<u8>>>,
    ops: Vec<WriteOp>,
}

impl<'env> RocksdbWriteTxn<'env> {
    pub(crate) fn new(inner: &'env Arc<RocksDbInner>) -> Self {
        let snapshot = inner.snapshot();
        Self {
            inner: Arc::clone(inner),
            snapshot,
            batch: WriteBatch::default(),
            buffers: RefCell::new(Vec::new()),
            ops: Vec::new(),
        }
    }

    fn cursor_cache<'txn>(&'txn self) -> CursorCache<'txn>
    where
        'env: 'txn,
    {
        CursorCache::new(&self.buffers)
    }

    fn lookup_overlay<'txn>(
        &'txn self,
        database: StoreDatabase,
        key: &[u8],
    ) -> Option<Option<&'txn [u8]>>
    where
        'env: 'txn,
    {
        for op in self.ops.iter().rev() {
            match op {
                WriteOp::Put {
                    database: db,
                    key: op_key,
                    value,
                } if *db == database && op_key.as_slice() == key => {
                    return Some(Some(self.cache_bytes(value.as_slice())));
                }
                WriteOp::Delete {
                    database: db,
                    key: op_key,
                } if *db == database && op_key.as_slice() == key => {
                    return Some(None);
                }
                WriteOp::Clear { database: db } if *db == database => {
                    return Some(None);
                }
                _ => {}
            }
        }
        None
    }

    fn cache_bytes<'txn>(&'txn self, data: &[u8]) -> &'txn [u8]
    where
        'env: 'txn,
    {
        let mut buffers = self.buffers.borrow_mut();
        buffers.push(data.to_vec());
        let idx = buffers.len() - 1;
        let ptr: *const Vec<u8> = &buffers[idx];
        drop(buffers);
        unsafe { (&*ptr).as_slice() }
    }

    fn apply_ops_to_map(
        &self,
        database: StoreDatabase,
        mut map: BTreeMap<Vec<u8>, Vec<u8>>,
    ) -> BTreeMap<Vec<u8>, Vec<u8>> {
        for op in &self.ops {
            match op {
                WriteOp::Put {
                    database: db,
                    key,
                    value,
                } if *db == database => {
                    map.insert(key.clone(), value.clone());
                }
                WriteOp::Delete { database: db, key } if *db == database => {
                    map.remove(key.as_slice());
                }
                WriteOp::Clear { database: db } if *db == database => {
                    map.clear();
                }
                _ => {}
            }
        }
        map
    }
}

impl<'env> StoreReadTxn<'env> for RocksdbWriteTxn<'env> {
    type Cursor<'txn>
        = RocksdbCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

    fn get<'txn>(&'txn self, database: StoreDatabase, key: &[u8]) -> StoreResult<&'txn [u8]>
    where
        'env: 'txn,
    {
        if let Some(result) = self.lookup_overlay(database, key) {
            return result.map_or(Err(StoreError::not_found()), Ok);
        }

        let handle = self.inner.cf_handle(database)?;
        match self
            .snapshot
            .get_pinned_cf(&handle, key)
            .map_err(store_error_from_rocksdb)?
        {
            Some(value) => Ok(self.cache_bytes(value.as_ref())),
            None => Err(StoreError::not_found()),
        }
    }

    fn count(&self, database: StoreDatabase) -> StoreResult<u64> {
        let map = self.inner.snapshot_entries_map(&self.snapshot, database)?;
        Ok(self.apply_ops_to_map(database, map).len() as u64)
    }

    fn open_cursor<'txn>(&'txn self, database: StoreDatabase) -> StoreResult<Self::Cursor<'txn>>
    where
        'env: 'txn,
    {
        let mut map = self.inner.snapshot_entries_map(&self.snapshot, database)?;
        map = self.apply_ops_to_map(database, map);
        let entries = map
            .into_iter()
            .map(|(k, v)| (k.into_boxed_slice(), v.into_boxed_slice()))
            .collect();
        Ok(RocksdbCursor::from_entries(self.cursor_cache(), entries))
    }

    fn commit(self) -> StoreResult<()>
    where
        Self: Sized,
    {
        self.inner
            .db
            .write(&self.batch)
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
        self.ops.push(WriteOp::Put {
            database,
            key: key.to_vec(),
            value: value.to_vec(),
        });
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
        self.ops.push(WriteOp::Delete {
            database,
            key: key.to_vec(),
        });
        Ok(())
    }

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        let handle = self.inner.cf_handle(database)?;
        let mut iter = self.inner.db.iterator_cf(&handle, IteratorMode::Start);
        while let Some(item) = iter.next() {
            let (key, _) = item.map_err(store_error_from_rocksdb)?;
            self.batch.delete_cf(&handle, &key);
        }
        self.ops.push(WriteOp::Clear { database });
        Ok(())
    }

    fn open_rw_cursor<'txn>(
        &'txn mut self,
        database: StoreDatabase,
    ) -> StoreResult<Self::MutCursor<'txn>>
    where
        'env: 'txn,
    {
        let mut map = self.inner.snapshot_entries_map(&self.snapshot, database)?;
        map = self.apply_ops_to_map(database, map);
        let entries = map
            .into_iter()
            .map(|(k, v)| (k.into_boxed_slice(), v.into_boxed_slice()))
            .collect();
        Ok(RocksdbCursor::from_entries(self.cursor_cache(), entries))
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        self.inner.delete_cf(database)
    }
}

enum WriteOp {
    Put {
        database: StoreDatabase,
        key: Vec<u8>,
        value: Vec<u8>,
    },
    Delete {
        database: StoreDatabase,
        key: Vec<u8>,
    },
    Clear {
        database: StoreDatabase,
    },
}

pub struct RocksdbCursor<'txn> {
    cache: CursorCache<'txn>,
    source: RocksdbCursorSource,
}

impl<'txn> RocksdbCursor<'txn> {
    fn from_entries(cache: CursorCache<'txn>, entries: Vec<(Box<[u8]>, Box<[u8]>)>) -> Self {
        Self {
            cache,
            source: RocksdbCursorSource::Buffered(entries.into_iter()),
        }
    }
}

impl<'txn> StoreCursor<'txn> for RocksdbCursor<'txn> {
    fn next(&mut self) -> StoreResult<Option<(&'txn [u8], &'txn [u8])>> {
        match &mut self.source {
            RocksdbCursorSource::Buffered(iter) => match iter.next() {
                Some((key, value)) => Ok(Some((
                    self.cache.cache_boxed_bytes(key),
                    self.cache.cache_boxed_bytes(value),
                ))),
                None => Ok(None),
            },
        }
    }
}

enum RocksdbCursorSource {
    Buffered(std::vec::IntoIter<(Box<[u8]>, Box<[u8]>)>),
}

struct CursorCache<'txn> {
    buffers: &'txn RefCell<Vec<Vec<u8>>>,
}

impl<'txn> CursorCache<'txn> {
    fn new(buffers: &'txn RefCell<Vec<Vec<u8>>>) -> Self {
        Self { buffers }
    }

    fn cache_boxed_bytes(&self, bytes: Box<[u8]>) -> &'txn [u8] {
        let vec: Vec<u8> = bytes.into();
        let mut buffers = self.buffers.borrow_mut();
        buffers.push(vec);
        let idx = buffers.len() - 1;
        let ptr: *const Vec<u8> = &buffers[idx];
        drop(buffers);
        unsafe { (&*ptr).as_slice() }
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
