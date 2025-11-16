use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    fs,
    io::Cursor,
    marker::PhantomData,
    mem,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Result, anyhow, bail};
use parking_lot::RwLock;
use rocksdb::{
    BoundColumnFamily, ColumnFamilyDescriptor, DBIteratorWithThreadMode, DBWithThreadMode,
    Error as RocksError, IteratorMode, MultiThreaded, Options, SnapshotWithThreadMode, WriteBatch,
};
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::{BlockHash, SavedBlock};
use store_traits::config::{LedgerBackend, LedgerStoreConfig, RocksDbConfig};
use store_traits::environment::{
    StoreCursor, StoreEnvironment, StoreEnvironmentFactory, StoreEnvironmentOptions, StoreReadTxn,
    StoreWriteTxn,
};
use store_traits::ledger::{
    LedgerCache, LedgerStore, LedgerStoreFactory, RangeBounds, StoreIterator,
};
use store_traits::transaction::{LedgerReadTxn, LedgerWriteTxn};
use store_traits::types::{
    StoreDatabase, StoreEnvironmentFlags, StoreError, StoreErrorKind, StoreResult, StoreRoCursor,
    StoreRwCursor, StoreWriteFlags,
};
use tempfile::tempdir;

pub struct RocksdbStoreEnvironment {
    inner: Arc<RocksDbInner>,
    _temp_dir: Option<tempfile::TempDir>,
}

impl RocksdbStoreEnvironment {
    fn open(
        path: PathBuf,
        _flags: StoreEnvironmentFlags,
        temp_dir: Option<tempfile::TempDir>,
        config: Option<&RocksDbConfig>,
    ) -> anyhow::Result<Self> {
        let inner = RocksDbInner::open(&path, config.and_then(|c| c.max_open_files))?;
        Ok(Self {
            inner: Arc::new(inner),
            _temp_dir: temp_dir,
        })
    }

    fn inner(&self) -> Arc<RocksDbInner> {
        Arc::clone(&self.inner)
    }
}

impl StoreEnvironment for RocksdbStoreEnvironment {
    type ReadTxn<'env>
        = RocksdbReadTxn<'env>
    where
        Self: 'env;
    type WriteTxn<'env>
        = RocksdbWriteTxn<'env>
    where
        Self: 'env;

    fn begin_read(&self) -> Self::ReadTxn<'_> {
        RocksdbReadTxn::new(&self.inner)
    }

    fn begin_write(&self) -> Self::WriteTxn<'_> {
        RocksdbWriteTxn::new(&self.inner)
    }

    fn open_db(&self, name: Option<&str>) -> StoreResult<StoreDatabase> {
        self.inner.open_database(name)
    }

    fn sync(&self) -> StoreResult<()> {
        self.inner.flush_wal()
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

    fn count(&self, database: StoreDatabase) -> u64 {
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

    fn count(&self, database: StoreDatabase) -> u64 {
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

    fn commit(self: Box<Self>) {
        self.inner.commit();
    }
}

pub struct RocksdbBlockStore {
    env: Arc<RocksdbStoreEnvironment>,
    index_cf: StoreDatabase,
    data_cf: StoreDatabase,
    put_listener: OutputListenerMt<SavedBlock>,
    next_id: AtomicU64,
}

impl RocksdbBlockStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let index_cf = env.open_db(Some(BLOCK_INDEX_CF_NAME))?;
        let data_cf = env.open_db(Some(BLOCK_DATA_CF_NAME))?;
        let next_id = find_next_block_id(&env, data_cf)?;
        Ok(Self {
            env,
            index_cf,
            data_cf,
            put_listener: OutputListenerMt::new(),
            next_id: AtomicU64::new(next_id),
        })
    }

    fn index_cf(&self) -> StoreDatabase {
        self.index_cf
    }

    fn data_cf(&self) -> StoreDatabase {
        self.data_cf
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<SavedBlock>> {
        self.put_listener.track()
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, block: &SavedBlock) {
        if self.put_listener.is_tracked() {
            self.put_listener.emit(block.clone());
        }

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let id_bytes = id.to_be_bytes();

        txn.put(
            self.index_cf(),
            block.hash().as_bytes(),
            &id_bytes,
            StoreWriteFlags::default(),
        )
        .expect("failed to write block index");

        txn.put(
            self.data_cf(),
            &id_bytes,
            &block.serialize_with_sideband(),
            StoreWriteFlags::default(),
        )
        .expect("failed to write block data");
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn, hash: &BlockHash) -> Option<SavedBlock> {
        let id_bytes = match txn.get(self.index_cf(), hash.as_bytes()) {
            Ok(bytes) => bytes,
            Err(e) if e.is_not_found() => return None,
            Err(e) => panic!("failed to read block index: {e}"),
        };
        self.load_block_bytes(txn, id_bytes)
    }

    pub fn exists(&self, txn: &dyn LedgerReadTxn, hash: &BlockHash) -> bool {
        txn.raw_exists(self.index_cf(), hash.as_bytes())
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, hash: &BlockHash) {
        let id = match txn.get(self.index_cf(), hash.as_bytes()) {
            Ok(bytes) => bytes,
            Err(e) if e.is_not_found() => return,
            Err(e) => panic!("failed to delete block: {e}"),
        };
        let id_vec = id.to_vec();
        txn.delete(self.data_cf(), &id_vec, None)
            .expect("failed to delete block data");
        txn.delete(self.index_cf(), hash.as_bytes(), None)
            .expect("failed to delete block index");
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.count(self.index_cf())
    }

    pub fn iter<'txn>(&'txn self, txn: &'txn dyn LedgerReadTxn) -> StoreIterator<'txn, SavedBlock> {
        let cursor = txn
            .open_ro_cursor(self.index_cf())
            .expect("failed to open block index cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbBlockIterator::new(cursor, txn, self.data_cf()))
    }

    pub fn iter_range<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
        range: RangeBounds<BlockHash>,
    ) -> StoreIterator<'txn, SavedBlock> {
        let cursor = txn
            .open_ro_cursor(self.index_cf())
            .expect("failed to open block index cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbBlockRangeIterator::new(
            cursor,
            txn,
            self.data_cf(),
            range,
        ))
    }

    fn load_block_bytes(&self, txn: &dyn LedgerReadTxn, id_bytes: &[u8]) -> Option<SavedBlock> {
        match txn.get(self.data_cf(), id_bytes) {
            Ok(data) => {
                let mut reader = Cursor::new(data.to_vec());
                Some(SavedBlock::deserialize(&mut reader).expect("failed to deserialize block"))
            }
            Err(e) if e.is_not_found() => None,
            Err(e) => panic!("failed to read block data: {e}"),
        }
    }
}

struct RocksdbBlockIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
    txn: &'txn dyn LedgerReadTxn,
    data_cf: StoreDatabase,
}

impl<'txn> RocksdbBlockIterator<'txn> {
    fn new(
        cursor: RocksdbCursor<'txn>,
        txn: &'txn dyn LedgerReadTxn,
        data_cf: StoreDatabase,
    ) -> Self {
        Self {
            cursor,
            txn,
            data_cf,
        }
    }
}

impl<'txn> Iterator for RocksdbBlockIterator<'txn> {
    type Item = SavedBlock;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let item = self
                .cursor
                .next()
                .expect("failed to advance RocksDB cursor");
            let (hash_bytes, id_bytes) = match item {
                Some(value) => value,
                None => return None,
            };
            let _hash =
                BlockHash::from_slice(hash_bytes).expect("invalid block hash bytes in RocksDB");
            let block = match self.txn.get(self.data_cf, id_bytes) {
                Ok(data) => {
                    let mut reader = Cursor::new(data.to_vec());
                    SavedBlock::deserialize(&mut reader)
                        .expect("failed to deserialize RocksDB block")
                }
                Err(e) if e.is_not_found() => continue,
                Err(e) => panic!("failed to load block data: {e}"),
            };
            return Some(block);
        }
    }
}

struct RocksdbBlockRangeIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
    txn: &'txn dyn LedgerReadTxn,
    data_cf: StoreDatabase,
    range: RangeBounds<BlockHash>,
}

impl<'txn> RocksdbBlockRangeIterator<'txn> {
    fn new(
        cursor: RocksdbCursor<'txn>,
        txn: &'txn dyn LedgerReadTxn,
        data_cf: StoreDatabase,
        range: RangeBounds<BlockHash>,
    ) -> Self {
        Self {
            cursor,
            txn,
            data_cf,
            range,
        }
    }
}

impl<'txn> Iterator for RocksdbBlockRangeIterator<'txn> {
    type Item = SavedBlock;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let item = self
                .cursor
                .next()
                .expect("failed to advance RocksDB cursor");
            let (hash_bytes, id_bytes) = match item {
                Some(value) => value,
                None => return None,
            };
            let hash =
                BlockHash::from_slice(hash_bytes).expect("invalid block hash bytes in RocksDB");
            if !hash_in_range(&hash, &self.range) {
                continue;
            }
            let block = match self.txn.get(self.data_cf, id_bytes) {
                Ok(data) => {
                    let mut reader = Cursor::new(data.to_vec());
                    SavedBlock::deserialize(&mut reader)
                        .expect("failed to deserialize RocksDB block")
                }
                Err(e) if e.is_not_found() => continue,
                Err(e) => panic!("failed to load block data: {e}"),
            };
            return Some(block);
        }
    }
}

fn find_next_block_id(env: &Arc<RocksdbStoreEnvironment>, data_cf: StoreDatabase) -> Result<u64> {
    let txn = RocksdbLedgerReadTxn::new(env);
    let cursor = txn
        .open_ro_cursor(data_cf)
        .map_err(|e| anyhow!(e.to_string()))?;
    let mut cursor = rocksdb_ro_cursor_from_store(cursor);
    let mut max_id: Option<u64> = None;
    loop {
        match cursor.next() {
            Ok(Some((key, _))) => {
                let id = u64::from_be_bytes(
                    key.try_into()
                        .map_err(|_| anyhow!("invalid block id bytes"))?,
                );
                max_id = Some(max_id.map_or(id, |current| current.max(id)));
            }
            Ok(None) => break,
            Err(e) => return Err(anyhow!(e.to_string())),
        }
    }
    Ok(max_id.map_or(0, |v| v + 1))
}

fn hash_in_range(hash: &BlockHash, range: &RangeBounds<BlockHash>) -> bool {
    use std::ops::Bound;
    let start_ok = match &range.start {
        Bound::Included(start) => hash >= start,
        Bound::Excluded(start) => hash > start,
        Bound::Unbounded => true,
    };
    let end_ok = match &range.end {
        Bound::Included(end) => hash <= end,
        Bound::Excluded(end) => hash < end,
        Bound::Unbounded => true,
    };
    start_ok && end_ok
}

pub struct RocksdbLedgerReadTxn {
    inner: RocksdbReadTxn<'static>,
}

impl RocksdbLedgerReadTxn {
    pub fn new(env: &Arc<RocksdbStoreEnvironment>) -> Self {
        let inner = env.inner();
        let txn = RocksdbReadTxn::new(&inner);
        let txn_static: RocksdbReadTxn<'static> = unsafe { mem::transmute(txn) };
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
        let txn_static: RocksdbWriteTxn<'static> = unsafe { mem::transmute(txn) };
        Self { inner: txn_static }
    }

    pub fn as_inner_mut(&mut self) -> &mut RocksdbWriteTxn<'static> {
        &mut self.inner
    }
}

pub struct RocksdbStoreEnvironmentFactory;

impl Default for RocksdbStoreEnvironmentFactory {
    fn default() -> Self {
        Self
    }
}

impl StoreEnvironmentFactory for RocksdbStoreEnvironmentFactory {
    type Environment = RocksdbStoreEnvironment;

    fn create(&self, options: StoreEnvironmentOptions) -> anyhow::Result<Arc<Self::Environment>> {
        let StoreEnvironmentOptions { path, flags, .. } = options;
        let env = RocksdbStoreEnvironment::open(path, flags, None, None)?;
        Ok(Arc::new(env))
    }

    fn create_null(&self) -> Arc<Self::Environment> {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir for rocksdb env");
        let options = StoreEnvironmentOptions {
            path: temp_dir.path().to_path_buf(),
            max_databases: 128,
            map_size: 0,
            flags: StoreEnvironmentFlags::empty(),
        };
        let StoreEnvironmentOptions { path, flags, .. } = options;
        let env = RocksdbStoreEnvironment::open(path, flags, Some(temp_dir), None)
            .expect("temp RocksDB environment");
        Arc::new(env)
    }
}

pub struct RocksdbLedgerStoreFactory;

impl Default for RocksdbLedgerStoreFactory {
    fn default() -> Self {
        Self
    }
}

impl RocksdbLedgerStoreFactory {
    pub fn new() -> Self {
        Self
    }
}

impl LedgerStoreFactory for RocksdbLedgerStoreFactory {
    fn create_store(
        &self,
        path: PathBuf,
        config: LedgerStoreConfig,
        _cache: Arc<LedgerCache>,
    ) -> anyhow::Result<Arc<dyn LedgerStore>> {
        let rocks_config = match config.backend {
            LedgerBackend::RocksDb(cfg) => cfg,
            _ => bail!("RocksDB factory requires RocksDB backend config"),
        };
        let _env = RocksdbStoreEnvironment::open(
            path,
            StoreEnvironmentFlags::empty(),
            None,
            Some(&rocks_config),
        )?;
        bail!("RocksDB ledger store not implemented yet")
    }

    fn create_null_store(&self, _cache: Arc<LedgerCache>) -> anyhow::Result<Arc<dyn LedgerStore>> {
        let temp_dir = tempdir()?;
        let _env = RocksdbStoreEnvironment::open(
            temp_dir.path().to_path_buf(),
            StoreEnvironmentFlags::empty(),
            Some(temp_dir),
            Some(&RocksDbConfig::default()),
        )?;
        bail!("RocksDB ledger store not implemented yet")
    }
}

struct RocksDbInner {
    db: RocksDb,
    registry: RwLock<CfRegistry>,
}

type RocksDb = DBWithThreadMode<MultiThreaded>;
type RocksDbSnapshot<'a> = SnapshotWithThreadMode<'a, RocksDb>;

const BLOCK_INDEX_CF_NAME: &str = "rocksdb_block_index";
const BLOCK_DATA_CF_NAME: &str = "rocksdb_block_data";

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

impl RocksDbInner {
    fn open(path: &Path, max_open_files: Option<i32>) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);
        if let Some(max_open_files) = max_open_files {
            options.set_max_open_files(max_open_files);
        }

        let cf_names = if path.exists() {
            RocksDb::list_cf(&options, path).unwrap_or_default()
        } else {
            Vec::new()
        };

        let db = if cf_names.is_empty() {
            let descriptor = ColumnFamilyDescriptor::new(
                rocksdb::DEFAULT_COLUMN_FAMILY_NAME,
                Options::default(),
            );
            RocksDb::open_cf_descriptors(&options, path, vec![descriptor])?
        } else {
            let descriptors = cf_names
                .iter()
                .map(|name| ColumnFamilyDescriptor::new(name, Options::default()))
                .collect::<Vec<_>>();
            RocksDb::open_cf_descriptors(&options, path, descriptors)?
        };

        let mut registry = CfRegistry::new();
        registry.insert_existing(rocksdb::DEFAULT_COLUMN_FAMILY_NAME);
        for name in cf_names {
            if name == rocksdb::DEFAULT_COLUMN_FAMILY_NAME {
                continue;
            }
            if db.cf_handle(&name).is_some() {
                registry.insert_existing(&name);
            }
        }

        Ok(Self {
            db,
            registry: RwLock::new(registry),
        })
    }

    fn snapshot(&self) -> RocksDbSnapshot<'_> {
        self.db.snapshot()
    }

    fn flush_wal(&self) -> StoreResult<()> {
        self.db.flush_wal(true).map_err(store_error_from_rocksdb)
    }

    fn open_database(&self, name: Option<&str>) -> StoreResult<StoreDatabase> {
        let name = name.unwrap_or(rocksdb::DEFAULT_COLUMN_FAMILY_NAME);
        if let Some(handle) = self.registry.read().handle_for_name(name) {
            return Ok(handle);
        }

        let mut cf_options = Options::default();
        cf_options.create_if_missing(true);
        self.db
            .create_cf(name, &cf_options)
            .map_err(store_error_from_rocksdb)?;
        self.db
            .cf_handle(name)
            .ok_or_else(|| StoreError::backend(format!("missing column family {name}")))?;
        Ok(self.registry.write().insert_existing(name))
    }

    fn cf_handle(&self, database: StoreDatabase) -> StoreResult<Arc<BoundColumnFamily<'_>>> {
        let name = {
            let registry = self.registry.read();
            registry
                .name_for_handle(database)
                .ok_or_else(|| StoreError::backend("invalid database handle"))?
        };
        self.db
            .cf_handle(&name)
            .ok_or_else(|| StoreError::backend(format!("missing column family {name}")))
    }

    fn collect_snapshot_entries(
        &self,
        snapshot: &RocksDbSnapshot<'_>,
        database: StoreDatabase,
    ) -> StoreResult<Vec<(Box<[u8]>, Box<[u8]>)>> {
        let handle = self.cf_handle(database)?;
        let iter = snapshot.iterator_cf(&handle, IteratorMode::Start);
        collect_entries(iter)
    }

    fn delete_cf(&self, database: StoreDatabase) -> StoreResult<()> {
        if database.into_raw().get() == 1 {
            return Err(StoreError::backend(
                "cannot drop default RocksDB column family",
            ));
        }

        let name = {
            let mut registry = self.registry.write();
            registry
                .remove(database)
                .ok_or_else(|| StoreError::backend("unknown column family"))?
        };
        self.db.drop_cf(&name).map_err(store_error_from_rocksdb)
    }
}

struct CfRegistry {
    next_id: usize,
    names_by_id: HashMap<usize, String>,
    ids_by_name: HashMap<String, usize>,
}

impl CfRegistry {
    fn new() -> Self {
        Self {
            next_id: 1,
            names_by_id: HashMap::new(),
            ids_by_name: HashMap::new(),
        }
    }

    fn insert_existing(&mut self, name: &str) -> StoreDatabase {
        if let Some(id) = self.ids_by_name.get(name) {
            return StoreDatabase::from_usize(*id).expect("non-zero id");
        }
        let id = self.next_id;
        self.next_id += 1;
        self.names_by_id.insert(id, name.to_string());
        self.ids_by_name.insert(name.to_string(), id);
        StoreDatabase::from_usize(id).expect("non-zero handle id")
    }

    fn handle_for_name(&self, name: &str) -> Option<StoreDatabase> {
        self.ids_by_name
            .get(name)
            .copied()
            .and_then(StoreDatabase::from_usize)
    }

    fn name_for_handle(&self, handle: StoreDatabase) -> Option<String> {
        let id = handle.into_raw().get();
        self.names_by_id.get(&id).cloned()
    }

    fn remove(&mut self, handle: StoreDatabase) -> Option<String> {
        let id = handle.into_raw().get();
        let name = self.names_by_id.remove(&id)?;
        self.ids_by_name.remove(&name);
        Some(name)
    }
}

pub struct RocksdbReadTxn<'env> {
    inner: Arc<RocksDbInner>,
    snapshot: RocksDbSnapshot<'env>,
    buffers: RefCell<Vec<Vec<u8>>>,
}

impl<'env> RocksdbReadTxn<'env> {
    fn new(inner: &'env Arc<RocksDbInner>) -> Self {
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

    fn count(&self, database: StoreDatabase) -> u64 {
        let entries = self
            .inner
            .collect_snapshot_entries(&self.snapshot, database)
            .unwrap_or_else(|e| panic!("failed to count RocksDB records: {e}"));
        entries.len() as u64
    }

    fn open_cursor<'txn>(&'txn self, database: StoreDatabase) -> StoreResult<Self::Cursor<'txn>>
    where
        'env: 'txn,
    {
        let entries = self
            .inner
            .collect_snapshot_entries(&self.snapshot, database)?;
        Ok(RocksdbCursor::new(entries))
    }

    fn commit(self)
    where
        Self: Sized,
    {
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
    fn new(inner: &'env Arc<RocksDbInner>) -> Self {
        let snapshot = inner.snapshot();
        Self {
            inner: Arc::clone(inner),
            snapshot,
            batch: WriteBatch::default(),
            buffers: RefCell::new(Vec::new()),
            ops: Vec::new(),
        }
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

    fn count(&self, database: StoreDatabase) -> u64 {
        let entries = self
            .inner
            .collect_snapshot_entries(&self.snapshot, database)
            .unwrap_or_else(|e| panic!("failed to count RocksDB records: {e}"));
        let map: BTreeMap<Vec<u8>, Vec<u8>> = entries
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        self.apply_ops_to_map(database, map).len() as u64
    }

    fn open_cursor<'txn>(&'txn self, database: StoreDatabase) -> StoreResult<Self::Cursor<'txn>>
    where
        'env: 'txn,
    {
        let base_entries = self
            .inner
            .collect_snapshot_entries(&self.snapshot, database)?;
        let mut map: BTreeMap<Vec<u8>, Vec<u8>> = base_entries
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        map = self.apply_ops_to_map(database, map);
        let entries = map
            .into_iter()
            .map(|(k, v)| (k.into_boxed_slice(), v.into_boxed_slice()))
            .collect();
        Ok(RocksdbCursor::new(entries))
    }

    fn commit(self)
    where
        Self: Sized,
    {
        if let Err(err) = self.inner.db.write(self.batch) {
            panic!("failed to commit RocksDB batch: {err}");
        }
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
        self.open_cursor(database)
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        self.inner.delete_cf(database)
    }
}

pub struct RocksdbCursor<'data> {
    entries: Vec<(Box<[u8]>, Box<[u8]>)>,
    index: usize,
    _marker: PhantomData<&'data ()>,
}

impl<'data> RocksdbCursor<'data> {
    fn new(entries: Vec<(Box<[u8]>, Box<[u8]>)>) -> Self {
        Self {
            entries,
            index: 0,
            _marker: PhantomData,
        }
    }
}

impl<'txn> StoreCursor<'txn> for RocksdbCursor<'txn> {
    fn next(&mut self) -> StoreResult<Option<(&'txn [u8], &'txn [u8])>> {
        if self.index >= self.entries.len() {
            return Ok(None);
        }
        let (key, value) = &self.entries[self.index];
        self.index += 1;
        let key_ref: &'txn [u8] = unsafe { mem::transmute::<&[u8], &'txn [u8]>(key.as_ref()) };
        let value_ref: &'txn [u8] = unsafe { mem::transmute::<&[u8], &'txn [u8]>(value.as_ref()) };
        Ok(Some((key_ref, value_ref)))
    }
}

fn store_ro_cursor_from_rocksdb<'txn>(cursor: RocksdbCursor<'txn>) -> StoreRoCursor<'txn> {
    let raw = Box::into_raw(Box::new(cursor)) as usize;
    let handle = unsafe { NonZeroUsize::new_unchecked(raw) };
    unsafe { StoreRoCursor::from_raw_parts(handle, drop_rocksdb_ro_cursor) }
}

fn store_rw_cursor_from_rocksdb<'txn>(cursor: RocksdbCursor<'txn>) -> StoreRwCursor<'txn> {
    let raw = Box::into_raw(Box::new(cursor)) as usize;
    let handle = unsafe { NonZeroUsize::new_unchecked(raw) };
    unsafe { StoreRwCursor::from_raw_parts(handle, drop_rocksdb_rw_cursor) }
}

fn rocksdb_ro_cursor_from_store<'txn>(cursor: StoreRoCursor<'txn>) -> RocksdbCursor<'txn> {
    let (handle, _) = cursor.into_raw_parts();
    let ptr = handle.get() as *mut RocksdbCursor<'txn>;
    *unsafe { Box::from_raw(ptr) }
}

fn rocksdb_rw_cursor_from_store<'txn>(cursor: StoreRwCursor<'txn>) -> RocksdbCursor<'txn> {
    let (handle, _) = cursor.into_raw_parts();
    let ptr = handle.get() as *mut RocksdbCursor<'txn>;
    *unsafe { Box::from_raw(ptr) }
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

fn collect_entries(
    iter: DBIteratorWithThreadMode<'_, RocksDb>,
) -> StoreResult<Vec<(Box<[u8]>, Box<[u8]>)>> {
    let mut entries = Vec::new();
    for item in iter {
        let (key, value) = item.map_err(store_error_from_rocksdb)?;
        entries.push((key, value));
    }
    Ok(entries)
}

fn store_error_from_rocksdb(err: RocksError) -> StoreError {
    let kind = match err.kind() {
        rocksdb::ErrorKind::NotFound => StoreErrorKind::NotFound,
        rocksdb::ErrorKind::InvalidArgument => StoreErrorKind::InvalidArgument,
        rocksdb::ErrorKind::Corruption => StoreErrorKind::Corruption,
        _ => StoreErrorKind::Backend,
    };
    StoreError::new(kind, err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BlockFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbBlockStore,
    }

    impl BlockFixture {
        fn new() -> Self {
            let dir = tempdir().unwrap();
            let env = Arc::new(
                RocksdbStoreEnvironment::open(
                    dir.path().to_path_buf(),
                    StoreEnvironmentFlags::empty(),
                    Some(dir),
                    Some(&RocksDbConfig::default()),
                )
                .unwrap(),
            );
            let store = RocksdbBlockStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }
    }

    fn create_env() -> Arc<RocksdbStoreEnvironment> {
        let dir = tempdir().unwrap();
        let env = RocksdbStoreEnvironment::open(
            dir.path().to_path_buf(),
            StoreEnvironmentFlags::empty(),
            Some(dir),
            Some(&RocksDbConfig::default()),
        )
        .unwrap();
        Arc::new(env)
    }

    #[test]
    fn write_and_read_roundtrip() {
        let env = create_env();
        let database = env.open_db(Some("blocks")).unwrap();

        {
            let mut txn = env.begin_write();
            txn.put(database, b"key", b"value", StoreWriteFlags::empty())
                .unwrap();
            txn.commit();
        }

        let txn = env.begin_read();
        assert_eq!(txn.get(database, b"key").unwrap(), b"value");
    }

    #[test]
    fn write_txn_reads_own_writes() {
        let env = create_env();
        let database = env.open_db(None).unwrap();

        let mut txn = env.begin_write();
        txn.put(database, b"pending", b"123", StoreWriteFlags::empty())
            .unwrap();
        assert_eq!(txn.get(database, b"pending").unwrap(), b"123");
    }

    #[test]
    fn cursor_reflects_overlay() {
        let env = create_env();
        let database = env.open_db(Some("accounts")).unwrap();

        let mut txn = env.begin_write();
        txn.put(database, b"a", b"1", StoreWriteFlags::empty())
            .unwrap();
        txn.put(database, b"b", b"2", StoreWriteFlags::empty())
            .unwrap();
        let mut cursor = txn.open_rw_cursor(database).unwrap();

        let first = cursor.next().unwrap().unwrap();
        assert_eq!(first, (b"a".as_ref(), b"1".as_ref()));
        let second = cursor.next().unwrap().unwrap();
        assert_eq!(second, (b"b".as_ref(), b"2".as_ref()));
    }

    #[test]
    fn block_store_put_get() {
        let fixture = BlockFixture::new();
        let block = SavedBlock::new_test_open_block();
        let mut write_txn = fixture.begin_write();
        fixture.store.put(&mut write_txn, &block);
        Box::new(write_txn).commit();

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &block.hash()), Some(block));
    }

    #[test]
    fn block_store_delete() {
        let fixture = BlockFixture::new();
        let block = SavedBlock::new_test_open_block();
        let mut write_txn = fixture.begin_write();
        fixture.store.put(&mut write_txn, &block);
        Box::new(write_txn).commit();

        let mut delete_txn = fixture.begin_write();
        fixture.store.del(&mut delete_txn, &block.hash());
        Box::new(delete_txn).commit();

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &block.hash()).is_none());
    }

    #[test]
    fn block_store_iterates() {
        let fixture = BlockFixture::new();
        let mut write_txn = fixture.begin_write();
        for _ in 0..3 {
            let block = SavedBlock::new_test_open_block();
            fixture.store.put(&mut write_txn, &block);
        }
        Box::new(write_txn).commit();

        let read_txn = fixture.begin_read();
        let count = fixture.store.iter(&read_txn).count();
        assert_eq!(count, 3);
    }
}
