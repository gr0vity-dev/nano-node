use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap, VecDeque},
    num::NonZeroUsize,
    sync::Arc,
};

use rocksdb::{DBIteratorWithThreadMode, IteratorMode, WriteBatch};
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
        let map = self.inner.snapshot_entries_map(&self.snapshot, database)?;
        let entries = map
            .into_iter()
            .map(|(k, v)| (StoreValue::from(k), StoreValue::from(v)))
            .collect();
        Ok(RocksdbCursor::from_entries(entries))
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
    ops: Vec<WriteOp>,
    overlays: HashMap<usize, ColumnOverlay>,
}

impl<'env> RocksdbWriteTxn<'env> {
    pub(crate) fn new(inner: &'env Arc<RocksDbInner>) -> Self {
        let snapshot = inner.snapshot();
        Self {
            inner: Arc::clone(inner),
            snapshot,
            batch: WriteBatch::default(),
            ops: Vec::new(),
            overlays: HashMap::new(),
        }
    }

    fn lookup_overlay<'txn>(
        &self,
        database: StoreDatabase,
        key: &[u8],
    ) -> Option<Option<StoreValue>> {
        if let Some(overlay) = self.column_overlay(database) {
            if let Some(entry) = overlay.get(key) {
                return match entry {
                    OverlayValue::Put(value) => Some(Some(StoreValue::from(value.clone()))),
                    OverlayValue::Delete => Some(None),
                };
            } else if overlay.is_cleared() {
                return Some(None);
            }
        }

        for op in self.ops.iter().rev() {
            match op {
                WriteOp::Put {
                    database: db,
                    key: op_key,
                    value,
                } if *db == database && op_key.as_slice() == key => {
                    return Some(Some(StoreValue::from(value.clone())));
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

    fn column_overlay(&self, database: StoreDatabase) -> Option<&ColumnOverlay> {
        let key = database.into_raw().get();
        self.overlays.get(&key)
    }

    fn column_overlay_mut(&mut self, database: StoreDatabase) -> &mut ColumnOverlay {
        let key = database.into_raw().get();
        self.overlays
            .entry(key)
            .or_insert_with(ColumnOverlay::default)
    }

    fn overlay_snapshot(&self, database: StoreDatabase) -> OverlaySnapshot {
        self.column_overlay(database)
            .map(ColumnOverlay::snapshot)
            .unwrap_or_default()
    }

    fn merge_iterator<'txn>(
        &'txn self,
        database: StoreDatabase,
    ) -> StoreResult<RocksdbMergeIterator<'txn>>
    where
        'env: 'txn,
    {
        let snapshot = self.overlay_snapshot(database);
        let (cleared, entries) = snapshot.into_parts();
        let snapshot_iter = if cleared {
            None
        } else {
            let handle = self.inner.cf_handle(database)?;
            Some(self.snapshot.iterator_cf(&handle, IteratorMode::Start))
        };
        Ok(RocksdbMergeIterator::new(snapshot_iter, entries))
    }
}

impl<'env> StoreReadTxn<'env> for RocksdbWriteTxn<'env> {
    type Cursor<'txn>
        = RocksdbCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<StoreValue> {
        if let Some(result) = self.lookup_overlay(database, key) {
            return result.map_or(Err(StoreError::not_found()), Ok);
        }

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
        let mut merge = self.merge_iterator(database)?;
        let mut count = 0u64;
        while merge.next_entry()?.is_some() {
            count += 1;
        }
        Ok(count)
    }

    fn open_cursor<'txn>(&'txn self, database: StoreDatabase) -> StoreResult<Self::Cursor<'txn>>
    where
        'env: 'txn,
    {
        let merge = self.merge_iterator(database)?;
        Ok(RocksdbCursor::streaming(merge))
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
        {
            let handle = self.inner.cf_handle(database)?;
            self.batch.put_cf(&handle, key, value);
        }
        self.column_overlay_mut(database)
            .insert_put(key.to_vec(), value.to_vec());
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
        {
            let handle = self.inner.cf_handle(database)?;
            self.batch.delete_cf(&handle, key);
        }
        self.column_overlay_mut(database)
            .insert_delete(key.to_vec());
        self.ops.push(WriteOp::Delete {
            database,
            key: key.to_vec(),
        });
        Ok(())
    }

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        {
            let handle = self.inner.cf_handle(database)?;
            let mut iter = self.inner.db.iterator_cf(&handle, IteratorMode::Start);
            while let Some(item) = iter.next() {
                let (key, _) = item.map_err(store_error_from_rocksdb)?;
                self.batch.delete_cf(&handle, &key);
            }
        }
        self.column_overlay_mut(database).clear_all();
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
        let merge = self.merge_iterator(database)?;
        Ok(RocksdbCursor::streaming(merge))
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        self.inner.delete_cf(database)
    }
}

#[derive(Default)]
struct ColumnOverlay {
    cleared: bool,
    entries: BTreeMap<Vec<u8>, OverlayValue>,
}

impl ColumnOverlay {
    fn insert_put(&mut self, key: Vec<u8>, value: Vec<u8>) {
        self.entries.insert(key, OverlayValue::Put(value));
    }

    fn insert_delete(&mut self, key: Vec<u8>) {
        self.entries.insert(key, OverlayValue::Delete);
    }

    fn clear_all(&mut self) {
        self.cleared = true;
        self.entries.clear();
    }

    fn get(&self, key: &[u8]) -> Option<&OverlayValue> {
        self.entries.get(key)
    }

    fn is_cleared(&self) -> bool {
        self.cleared
    }

    fn snapshot(&self) -> OverlaySnapshot {
        let entries = self
            .entries
            .iter()
            .map(|(key, value)| OverlayEntry {
                key: key.clone(),
                value: value.clone(),
            })
            .collect::<VecDeque<_>>();
        OverlaySnapshot::new(self.cleared, entries)
    }
}

#[derive(Default)]
struct OverlaySnapshot {
    cleared: bool,
    entries: VecDeque<OverlayEntry>,
}

impl OverlaySnapshot {
    fn new(cleared: bool, entries: VecDeque<OverlayEntry>) -> Self {
        Self { cleared, entries }
    }

    fn into_parts(self) -> (bool, VecDeque<OverlayEntry>) {
        (self.cleared, self.entries)
    }
}

struct OverlayEntry {
    key: Vec<u8>,
    value: OverlayValue,
}

#[derive(Clone)]
enum OverlayValue {
    Put(Vec<u8>),
    Delete,
}

struct RocksdbMergeIterator<'txn> {
    snapshot_iter: Option<DBIteratorWithThreadMode<'txn, RocksDb>>,
    snapshot_peeked: Option<(Vec<u8>, Vec<u8>)>,
    overlay_entries: VecDeque<OverlayEntry>,
}

impl<'txn> RocksdbMergeIterator<'txn> {
    fn new(
        snapshot_iter: Option<DBIteratorWithThreadMode<'txn, RocksDb>>,
        overlay_entries: VecDeque<OverlayEntry>,
    ) -> Self {
        Self {
            snapshot_iter,
            snapshot_peeked: None,
            overlay_entries,
        }
    }

    fn next_entry(&mut self) -> StoreResult<Option<(Vec<u8>, Vec<u8>)>> {
        loop {
            self.ensure_snapshot_peeked()?;
            let overlay_key = self
                .overlay_entries
                .front()
                .map(|entry| entry.key.as_slice());
            let snapshot_key = self.snapshot_peeked.as_ref().map(|(key, _)| key.as_slice());

            match (overlay_key, snapshot_key) {
                (None, None) => return Ok(None),
                (Some(_), None) => {
                    let entry = self.overlay_entries.pop_front().expect("entry present");
                    if let OverlayValue::Put(value) = entry.value {
                        return Ok(Some((entry.key, value)));
                    }
                }
                (None, Some(_)) => {
                    let entry = self.snapshot_peeked.take().expect("snapshot entry present");
                    return Ok(Some(entry));
                }
                (Some(overlay_key), Some(snapshot_key)) => match overlay_key.cmp(snapshot_key) {
                    Ordering::Less => {
                        let entry = self.overlay_entries.pop_front().expect("entry present");
                        if let OverlayValue::Put(value) = entry.value {
                            return Ok(Some((entry.key, value)));
                        }
                    }
                    Ordering::Equal => {
                        let entry = self.overlay_entries.pop_front().expect("entry present");
                        self.snapshot_peeked.take();
                        if let OverlayValue::Put(value) = entry.value {
                            return Ok(Some((entry.key, value)));
                        }
                    }
                    Ordering::Greater => {
                        let entry = self.snapshot_peeked.take().expect("snapshot entry present");
                        return Ok(Some(entry));
                    }
                },
            }
        }
    }

    fn ensure_snapshot_peeked(&mut self) -> StoreResult<()> {
        if self.snapshot_peeked.is_some() || self.snapshot_iter.is_none() {
            return Ok(());
        }

        if let Some(iter) = &mut self.snapshot_iter {
            if let Some(item) = iter.next() {
                let (key, value) = item.map_err(store_error_from_rocksdb)?;
                self.snapshot_peeked = Some((key.into(), value.into()));
            } else {
                self.snapshot_iter = None;
            }
        }
        Ok(())
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
    source: RocksdbCursorSource<'txn>,
}

impl<'txn> RocksdbCursor<'txn> {
    fn from_entries(entries: Vec<(StoreValue, StoreValue)>) -> Self {
        Self {
            source: RocksdbCursorSource::Buffered(entries.into_iter()),
        }
    }

    fn streaming(merge: RocksdbMergeIterator<'txn>) -> Self {
        Self {
            source: RocksdbCursorSource::Streaming(merge),
        }
    }
}

impl<'txn> StoreCursor<'txn> for RocksdbCursor<'txn> {
    fn next(&mut self) -> StoreResult<Option<(StoreValue, StoreValue)>> {
        match &mut self.source {
            RocksdbCursorSource::Buffered(iter) => Ok(iter.next()),
            RocksdbCursorSource::Streaming(iter) => {
                Ok(iter.next_entry()?.map(|(key, value)| (key.into(), value.into())))
            }
        }
    }
}

enum RocksdbCursorSource<'txn> {
    Buffered(std::vec::IntoIter<(StoreValue, StoreValue)>),
    Streaming(RocksdbMergeIterator<'txn>),
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
