pub mod account_store;
pub mod block_store;
pub mod confirmation_height_store;
pub mod final_vote_store;
pub mod online_weight_store;
pub mod peer_store;
pub mod pending_store;
pub mod pruned_store;
pub mod rep_weight_store;
pub mod successor_store;
pub mod version_store;

pub use account_store::RocksdbAccountStore;
pub use block_store::RocksdbBlockStore;
pub use confirmation_height_store::RocksdbConfirmationHeightStore;
pub use final_vote_store::RocksdbFinalVoteStore;
pub use online_weight_store::RocksdbOnlineWeightStore;
pub use peer_store::RocksdbPeerStore;
pub use pending_store::RocksdbPendingStore;
pub use pruned_store::RocksdbPrunedStore;
pub use rep_weight_store::RocksdbRepWeightStore;
pub use successor_store::RocksdbSuccessorStore;
pub use version_store::RocksdbVersionStore;

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    fs, mem,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    slice,
    sync::{Arc, OnceLock},
};

#[cfg(test)]
#[path = "../build_support.rs"]
mod build_support;

use anyhow::{Result, anyhow, bail};
use parking_lot::RwLock;
use rocksdb::{
    BoundColumnFamily, ColumnFamilyDescriptor, DBIteratorWithThreadMode, DBWithThreadMode,
    Error as RocksError, IteratorMode, MultiThreaded, Options, SnapshotWithThreadMode, WriteBatch,
};
use rsnano_types::{Account, AccountInfo, ConfirmationHeightInfo};
use store_traits::config::{LedgerBackend, LedgerStoreConfig, RocksDbConfig};
use store_traits::environment::{
    StoreCursor, StoreEnvironment, StoreEnvironmentFactory, StoreEnvironmentOptions, StoreReadTxn,
    StoreWriteTxn,
};
use store_traits::ledger::{
    AccountStore, BlockStore, ConfirmationHeightStore, FinalVoteStore, LedgerCache, LedgerStore,
    LedgerStoreFactory, MemoryStats, OnlineWeightStore, PeerStore, PendingStore, RangeBounds,
    RepWeightStore, StoreVendor, SuccessorStore, VersionStore,
};
use store_traits::transaction::{LedgerReadTxn, LedgerWriteTxn};
use store_traits::types::{
    StoreDatabase, StoreEnvironmentFlags, StoreError, StoreErrorKind, StoreResult, StoreRoCursor,
    StoreRwCursor, StoreWriteFlags,
};

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
    ) -> Result<Self> {
        let inner = RocksDbInner::open(&path, config.and_then(|c| c.max_open_files))?;
        Ok(Self {
            inner: Arc::new(inner),
            _temp_dir: temp_dir,
        })
    }

    fn inner(&self) -> Arc<RocksDbInner> {
        Arc::clone(&self.inner)
    }

    pub fn open_db(&self, name: Option<&str>) -> StoreResult<StoreDatabase> {
        self.inner.open_database(name)
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

struct RocksdbLedgerStore {
    env: Arc<RocksdbStoreEnvironment>,
    cache: Arc<LedgerCache>,
    block: RocksdbBlockStore,
    account: RocksdbAccountStore,
    pending: RocksdbPendingStore,
    confirmation_height: RocksdbConfirmationHeightStore,
    rep_weight: Arc<RocksdbRepWeightStore>,
    successors: RocksdbSuccessorStore,
    final_vote: RocksdbFinalVoteStore,
    peer: RocksdbPeerStore,
    version: RocksdbVersionStore,
    online_weight: RocksdbOnlineWeightStore,
}

impl RocksdbLedgerStore {
    fn create(
        env: Arc<RocksdbStoreEnvironment>,
        cache: Arc<LedgerCache>,
    ) -> anyhow::Result<Arc<dyn LedgerStore>> {
        let block = RocksdbBlockStore::new(Arc::clone(&env))?;
        let account = RocksdbAccountStore::new(Arc::clone(&env))?;
        let pending = RocksdbPendingStore::new(Arc::clone(&env))?;
        let confirmation_height = RocksdbConfirmationHeightStore::new(Arc::clone(&env))?;
        let rep_weight = Arc::new(RocksdbRepWeightStore::new(Arc::clone(&env))?);
        let successors = RocksdbSuccessorStore::new(Arc::clone(&env))?;
        let online_weight = RocksdbOnlineWeightStore::new(Arc::clone(&env))?;
        let final_vote = RocksdbFinalVoteStore::new(Arc::clone(&env))?;
        let peer = RocksdbPeerStore::new(Arc::clone(&env))?;
        let version = RocksdbVersionStore::new(Arc::clone(&env))?;

        Ok(Arc::new(Self {
            env,
            cache,
            block,
            account,
            pending,
            confirmation_height,
            rep_weight,
            successors,
            final_vote,
            peer,
            version,
            online_weight,
        }))
    }
}

impl LedgerStore for RocksdbLedgerStore {
    fn block_store(&self) -> &dyn BlockStore {
        &self.block
    }

    fn account_store(&self) -> &dyn AccountStore {
        &self.account
    }

    fn pending_store(&self) -> &dyn PendingStore {
        &self.pending
    }

    fn confirmation_height_store(&self) -> &dyn ConfirmationHeightStore {
        &self.confirmation_height
    }

    fn successor_store(&self) -> &dyn SuccessorStore {
        &self.successors
    }

    fn final_vote_store(&self) -> &dyn FinalVoteStore {
        &self.final_vote
    }

    fn peer_store(&self) -> &dyn PeerStore {
        &self.peer
    }

    fn version_store(&self) -> &dyn VersionStore {
        &self.version
    }

    fn online_weight_store(&self) -> &dyn OnlineWeightStore {
        &self.online_weight
    }

    fn rep_weight_store(&self) -> Arc<dyn RepWeightStore> {
        self.rep_weight.clone()
    }

    fn begin_read(&self) -> Box<dyn LedgerReadTxn> {
        Box::new(RocksdbLedgerReadTxn::new(&self.env))
    }

    fn begin_write(&self) -> Box<dyn LedgerWriteTxn> {
        Box::new(RocksdbLedgerWriteTxn::new(&self.env))
    }

    fn sync(&self) -> anyhow::Result<()> {
        self.env.sync().map_err(|e| anyhow!(e.to_string()))
    }

    fn cache(&self) -> &LedgerCache {
        &self.cache
    }

    fn memory_stats(&self) -> anyhow::Result<MemoryStats> {
        Ok(MemoryStats {
            branch_pages: 0,
            depth: 0,
            entries: 0,
            leaf_pages: 0,
            overflow_pages: 0,
            page_size: 0,
        })
    }

    fn for_each_account_par(
        &self,
        _thread_count: usize,
        action: &(dyn Fn(&mut dyn Iterator<Item = (Account, AccountInfo)>) + Send + Sync),
    ) {
        let txn = RocksdbLedgerReadTxn::new(&self.env);
        let mut iter = self.account.iter(&txn);
        action(&mut iter);
    }

    fn for_each_confirmation_height_par(
        &self,
        _thread_count: usize,
        action: &(
             dyn Fn(&mut dyn Iterator<Item = (Account, ConfirmationHeightInfo)>) + Send + Sync
         ),
    ) {
        let txn = RocksdbLedgerReadTxn::new(&self.env);
        let mut iter = self.confirmation_height.iter(&txn);
        action(&mut iter);
    }

    fn vendor(&self) -> StoreVendor {
        rocksdb_vendor()
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

fn value_in_range<T>(value: &T, range: &RangeBounds<T>) -> bool
where
    T: Ord,
{
    use std::ops::Bound;
    let start_ok = match &range.start {
        Bound::Included(start) => value >= start,
        Bound::Excluded(start) => value > start,
        Bound::Unbounded => true,
    };
    let end_ok = match &range.end {
        Bound::Included(end) => value <= end,
        Bound::Excluded(end) => value < end,
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
        cache: Arc<LedgerCache>,
    ) -> anyhow::Result<Arc<dyn LedgerStore>> {
        let rocks_config = match config.backend {
            LedgerBackend::RocksDb(cfg) => cfg,
            _ => bail!("RocksDB factory requires RocksDB backend config"),
        };
        let env = RocksdbStoreEnvironment::open(
            path,
            StoreEnvironmentFlags::empty(),
            None,
            Some(&rocks_config),
        )?;
        RocksdbLedgerStore::create(Arc::new(env), cache)
    }

    fn create_null_store(&self, cache: Arc<LedgerCache>) -> anyhow::Result<Arc<dyn LedgerStore>> {
        let env_factory = RocksdbStoreEnvironmentFactory::default();
        let env = env_factory.create_null();
        RocksdbLedgerStore::create(env, cache)
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
const ACCOUNTS_CF_NAME: &str = "rocksdb_accounts";
const PENDING_CF_NAME: &str = "rocksdb_pending";
const CONF_HEIGHT_CF_NAME: &str = "rocksdb_confirmation_height";
const REP_WEIGHT_CF_NAME: &str = "rocksdb_rep_weights";
const SUCCESSOR_CF_NAME: &str = "rocksdb_successors";
const ONLINE_WEIGHT_CF_NAME: &str = "rocksdb_online_weight";
const PRUNED_CF_NAME: &str = "rocksdb_pruned";
const FINAL_VOTE_CF_NAME: &str = "rocksdb_final_votes";
const PEERS_CF_NAME: &str = "rocksdb_peers";
const VERSION_CF_NAME: &str = "rocksdb_version";

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

    fn count_snapshot_entries(
        &self,
        snapshot: &RocksDbSnapshot<'_>,
        database: StoreDatabase,
    ) -> StoreResult<u64> {
        let handle = self.cf_handle(database)?;
        let mut iter = snapshot.iterator_cf(&handle, IteratorMode::Start);
        let mut count = 0u64;
        while let Some(item) = iter.next() {
            item.map_err(store_error_from_rocksdb)?;
            count += 1;
        }
        Ok(count)
    }

    fn snapshot_entries_map(
        &self,
        snapshot: &RocksDbSnapshot<'_>,
        database: StoreDatabase,
    ) -> StoreResult<BTreeMap<Vec<u8>, Vec<u8>>> {
        let handle = self.cf_handle(database)?;
        let mut iter = snapshot.iterator_cf(&handle, IteratorMode::Start);
        let mut map = BTreeMap::new();
        while let Some(item) = iter.next() {
            let (key, value) = item.map_err(store_error_from_rocksdb)?;
            map.insert(key.into(), value.into());
        }
        Ok(map)
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
        let handle = self.inner.cf_handle(database)?;
        let iter = self.snapshot.iterator_cf(&handle, IteratorMode::Start);
        Ok(RocksdbCursor::streaming(self.cursor_cache(), iter))
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
    count_trackers: RefCell<HashMap<usize, CountTracker>>,
}

#[derive(Default)]
struct CountTracker {
    base_count: Option<u64>,
    delta: i64,
    key_states: HashMap<Vec<u8>, KeyCountState>,
    cleared: bool,
}

#[derive(Clone)]
struct KeyCountState {
    current_present: bool,
}

fn bool_to_i64(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

fn database_key(database: StoreDatabase) -> usize {
    database.into_raw().get()
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
            count_trackers: RefCell::new(HashMap::new()),
        }
    }

    fn ensure_base_count(
        &self,
        tracker: &mut CountTracker,
        database: StoreDatabase,
    ) -> StoreResult<()> {
        if tracker.base_count.is_none() {
            let count = if tracker.cleared {
                0
            } else {
                self.inner
                    .count_snapshot_entries(&self.snapshot, database)?
            };
            tracker.base_count = Some(count);
        }
        Ok(())
    }

    fn snapshot_contains(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<bool> {
        let handle = self.inner.cf_handle(database)?;
        let exists = self
            .snapshot
            .get_pinned_cf(&handle, key)
            .map_err(store_error_from_rocksdb)?
            .is_some();
        Ok(exists)
    }

    fn ensure_key_state<'a>(
        &'a self,
        tracker: &'a mut CountTracker,
        database: StoreDatabase,
        key: &[u8],
    ) -> StoreResult<&'a mut KeyCountState> {
        if tracker.key_states.contains_key(key) {
            return Ok(tracker
                .key_states
                .get_mut(key)
                .expect("key state present by contains_key"));
        }

        let initial_present = if tracker.cleared {
            false
        } else {
            self.snapshot_contains(database, key)?
        };

        tracker.key_states.insert(
            key.to_vec(),
            KeyCountState {
                current_present: initial_present,
            },
        );
        Ok(tracker
            .key_states
            .get_mut(key)
            .expect("key state inserted above"))
    }

    fn update_key_presence(
        &self,
        database: StoreDatabase,
        key: &[u8],
        present: bool,
    ) -> StoreResult<()> {
        let mut trackers = self.count_trackers.borrow_mut();
        let tracker = trackers.entry(database_key(database)).or_default();
        let delta_change = {
            let state = self.ensure_key_state(tracker, database, key)?;
            if state.current_present == present {
                0
            } else {
                let change = bool_to_i64(present) - bool_to_i64(state.current_present);
                state.current_present = present;
                change
            }
        };
        if delta_change != 0 {
            tracker.delta += delta_change;
        }
        Ok(())
    }

    fn handle_clear_tracker(&self, database: StoreDatabase) {
        let mut trackers = self.count_trackers.borrow_mut();
        let tracker = trackers.entry(database_key(database)).or_default();
        tracker.base_count = Some(0);
        tracker.delta = 0;
        tracker.key_states.clear();
        tracker.cleared = true;
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

    fn cursor_cache<'txn>(&'txn self) -> CursorCache<'txn>
    where
        'env: 'txn,
    {
        CursorCache::new(&self.buffers)
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
        let mut trackers = self.count_trackers.borrow_mut();
        let tracker = trackers.entry(database_key(database)).or_default();
        self.ensure_base_count(tracker, database)?;
        let base = tracker.base_count.expect("base count ensured");
        let total = (base as i128) + tracker.delta as i128;
        debug_assert!(total >= 0, "store count went negative");
        Ok(total as u64)
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
            .write(self.batch)
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
        self.update_key_presence(database, key, true)?;
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
        self.update_key_presence(database, key, false)?;
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
        self.handle_clear_tracker(database);
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

enum RocksdbCursorSource<'txn> {
    Streaming(DBIteratorWithThreadMode<'txn, RocksDb>),
    Buffered(std::vec::IntoIter<(Box<[u8]>, Box<[u8]>)>),
}

pub struct RocksdbCursor<'txn> {
    cache: CursorCache<'txn>,
    source: RocksdbCursorSource<'txn>,
}

impl<'txn> RocksdbCursor<'txn> {
    fn streaming(cache: CursorCache<'txn>, iter: DBIteratorWithThreadMode<'txn, RocksDb>) -> Self {
        Self {
            cache,
            source: RocksdbCursorSource::Streaming(iter),
        }
    }

    fn from_entries(cache: CursorCache<'txn>, entries: Vec<(Box<[u8]>, Box<[u8]>)>) -> Self {
        Self {
            cache,
            source: RocksdbCursorSource::Buffered(entries.into_iter()),
        }
    }

    fn cache_pair(&self, key: Box<[u8]>, value: Box<[u8]>) -> (&'txn [u8], &'txn [u8]) {
        let key_ref = self.cache.cache_boxed(key);
        let value_ref = self.cache.cache_boxed(value);
        (key_ref, value_ref)
    }
}

impl<'txn> StoreCursor<'txn> for RocksdbCursor<'txn> {
    fn next(&mut self) -> StoreResult<Option<(&'txn [u8], &'txn [u8])>> {
        match &mut self.source {
            RocksdbCursorSource::Streaming(iter) => match iter.next() {
                Some(item) => {
                    let (key, value) = item.map_err(store_error_from_rocksdb)?;
                    let (key_ref, value_ref) = self.cache_pair(key, value);
                    Ok(Some((key_ref, value_ref)))
                }
                None => Ok(None),
            },
            RocksdbCursorSource::Buffered(iter) => match iter.next() {
                Some((key, value)) => {
                    let (key_ref, value_ref) = self.cache_pair(key, value);
                    Ok(Some((key_ref, value_ref)))
                }
                None => Ok(None),
            },
        }
    }
}

struct CursorCache<'txn> {
    buffers: &'txn RefCell<Vec<Vec<u8>>>,
}

impl<'txn> CursorCache<'txn> {
    fn new(buffers: &'txn RefCell<Vec<Vec<u8>>>) -> Self {
        Self { buffers }
    }

    fn cache_boxed(&self, data: Box<[u8]>) -> &'txn [u8] {
        self.cache_vec(data.into_vec())
    }

    fn cache_vec(&self, data: Vec<u8>) -> &'txn [u8] {
        let mut buffers = self.buffers.borrow_mut();
        buffers.push(data);
        let idx = buffers.len() - 1;
        let slice_ref = buffers[idx].as_slice();
        let ptr = slice_ref.as_ptr();
        let len = slice_ref.len();
        drop(buffers);
        unsafe { slice::from_raw_parts(ptr, len) }
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

fn rocksdb_vendor() -> StoreVendor {
    static VENDOR: OnceLock<StoreVendor> = OnceLock::new();
    VENDOR
        .get_or_init(|| {
            let version = option_env!("RSN_ROCKSDB_LIB_VERSION").unwrap_or("unknown");
            StoreVendor::new("rocksdb", version)
        })
        .clone()
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
    use rsnano_types::{
        Amount, Block, BlockHash, PendingInfo, PendingKey, PrivateKey, PublicKey, QualifiedRoot,
        SavedBlock,
    };
    use std::{
        fs,
        net::{Ipv6Addr, SocketAddrV6},
        ops::Bound,
        time::{Duration, UNIX_EPOCH},
    };
    use tempfile::tempdir;

    #[test]
    fn vendor_matches_librocksdb_version() {
        let vendor = rocksdb_vendor();
        assert_eq!(vendor.name, "rocksdb");

        let lock_path = build_support::workspace_lock_path().expect("workspace Cargo.lock");
        let contents = fs::read_to_string(lock_path).expect("read Cargo.lock");
        let version = build_support::find_version(&contents, "librocksdb-sys")
            .expect("librocksdb-sys version");
        let expected = version
            .split('+')
            .nth(1)
            .unwrap_or(version.as_str())
            .to_string();

        assert_eq!(vendor.version, expected);
    }

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

    struct AccountFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbAccountStore,
    }

    impl AccountFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbAccountStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_accounts(&self, entries: &[(Account, AccountInfo)]) {
            let mut txn = self.begin_write();
            for (account, info) in entries {
                self.store.put(&mut txn, account, info);
            }
            Box::new(txn).commit().expect("rocksdb test commit failed");
        }
    }

    struct PendingFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbPendingStore,
    }

    impl PendingFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbPendingStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_entries(&self, entries: &[(PendingKey, PendingInfo)]) {
            let mut txn = self.begin_write();
            for (key, info) in entries {
                self.store.put(&mut txn, key, info);
            }
            Box::new(txn).commit().expect("rocksdb test commit failed");
        }
    }

    struct ConfirmationFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbConfirmationHeightStore,
    }

    impl ConfirmationFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbConfirmationHeightStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_entries(&self, entries: &[(Account, ConfirmationHeightInfo)]) {
            let mut txn = self.begin_write();
            for (account, info) in entries {
                self.store.put(&mut txn, account, info);
            }
            Box::new(txn).commit().expect("rocksdb test commit failed");
        }
    }

    struct RepWeightFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbRepWeightStore,
    }

    #[test]
    fn write_txn_count_tracks_overlay_changes() {
        let fixture = AccountFixture::new();
        let mut txn = fixture.begin_write();
        let account = Account::from(1);
        let info = AccountInfo::new_test_instance();

        assert_eq!(fixture.store.count(&txn), 0);

        fixture.store.put(&mut txn, &account, &info);
        assert_eq!(fixture.store.count(&txn), 1);

        let info2 = AccountInfo::new_test_instance();
        fixture.store.put(&mut txn, &account, &info2);
        assert_eq!(fixture.store.count(&txn), 1);

        fixture.store.del(&mut txn, &account);
        assert_eq!(fixture.store.count(&txn), 0);
    }

    #[test]
    fn write_txn_count_handles_clear() {
        let fixture = ConfirmationFixture::new();
        let entries = vec![
            (
                Account::from(1),
                ConfirmationHeightInfo::new(1, BlockHash::from(10)),
            ),
            (
                Account::from(2),
                ConfirmationHeightInfo::new(2, BlockHash::from(20)),
            ),
        ];
        fixture.insert_entries(&entries);

        let mut txn = fixture.begin_write();
        assert_eq!(fixture.store.count(&txn), entries.len() as u64);

        fixture.store.clear(&mut txn);
        assert_eq!(fixture.store.count(&txn), 0);

        let account = Account::from(3);
        let info = ConfirmationHeightInfo::new(5, BlockHash::from(30));
        fixture.store.put(&mut txn, &account, &info);
        assert_eq!(fixture.store.count(&txn), 1);
    }

    impl RepWeightFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbRepWeightStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_entries(&self, entries: &[(PublicKey, Amount)]) {
            let mut txn = self.begin_write();
            for (account, weight) in entries {
                self.store.put(&mut txn, *account, *weight);
            }
            Box::new(txn).commit().expect("rocksdb test commit failed");
        }
    }

    struct SuccessorFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbSuccessorStore,
    }

    impl SuccessorFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbSuccessorStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_entries(&self, entries: &[(BlockHash, BlockHash)]) {
            let mut txn = self.begin_write();
            for (block, successor) in entries {
                self.store.put(&mut txn, block, successor);
            }
            Box::new(txn).commit().expect("rocksdb test commit failed");
        }
    }

    struct OnlineWeightFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbOnlineWeightStore,
    }

    impl OnlineWeightFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbOnlineWeightStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_entries(&self, entries: &[(u64, Amount)]) {
            let mut txn = self.begin_write();
            for (time, amount) in entries {
                self.store.put(&mut txn, *time, amount);
            }
            Box::new(txn).commit().expect("rocksdb test commit failed");
        }
    }

    struct PrunedFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbPrunedStore,
    }

    impl PrunedFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbPrunedStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }
    }

    struct FinalVoteFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbFinalVoteStore,
    }

    impl FinalVoteFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbFinalVoteStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }
    }

    struct PeerFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbPeerStore,
    }

    impl PeerFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbPeerStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }
    }

    struct VersionFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbVersionStore,
    }

    impl VersionFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbVersionStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }
    }

    #[test]
    fn write_and_read_roundtrip() {
        let env = create_env();
        let database = env.open_db(Some("blocks")).unwrap();

        {
            let mut txn = env.begin_write();
            txn.put(database, b"key", b"value", StoreWriteFlags::empty())
                .unwrap();
            txn.commit().expect("rocksdb write txn commit failed");
        }

        let txn = env.begin_read();
        assert_eq!(txn.get(database, b"key").unwrap(), b"value");
    }

    #[test]
    fn write_txn_drop_discards_changes() {
        let env = create_env();
        let database = env.open_db(Some("accounts")).unwrap();

        {
            let mut txn = env.begin_write();
            txn.put(database, b"key", b"value", StoreWriteFlags::empty())
                .unwrap();
            // Transaction dropped without commit
        }

        let read_txn = env.begin_read();
        let err = read_txn.get(database, b"key").unwrap_err();
        assert!(err.is_not_found());
    }

    #[test]
    fn write_txn_explicit_rollback() {
        let env = create_env();
        let database = env.open_db(Some("accounts")).unwrap();
        let mut txn = env.begin_write();
        txn.put(database, b"rollback", b"value", StoreWriteFlags::empty())
            .unwrap();
        drop(txn);

        let read_txn = env.begin_read();
        assert!(read_txn.get(database, b"rollback").is_err());
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
    fn read_txn_snapshot_isolation() {
        let env = create_env();
        let database = env.open_db(Some("pending")).unwrap();

        {
            let mut txn = env.begin_write();
            txn.put(database, b"snapshot", b"v1", StoreWriteFlags::empty())
                .unwrap();
            txn.commit().expect("rocksdb write txn commit failed");
        }

        let read_txn = env.begin_read();
        assert_eq!(read_txn.get(database, b"snapshot").unwrap(), b"v1");

        {
            let mut write_txn = env.begin_write();
            write_txn
                .put(database, b"snapshot", b"v2", StoreWriteFlags::empty())
                .unwrap();
            write_txn.commit().expect("rocksdb write txn commit failed");
        }

        // Existing read transaction should continue to see the original value.
        assert_eq!(read_txn.get(database, b"snapshot").unwrap(), b"v1");

        // A fresh read transaction gets the updated value.
        let fresh_read = env.begin_read();
        assert_eq!(fresh_read.get(database, b"snapshot").unwrap(), b"v2");
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
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &block.hash()), Some(block));
    }

    #[test]
    fn block_store_delete() {
        let fixture = BlockFixture::new();
        let block = SavedBlock::new_test_open_block();
        let mut write_txn = fixture.begin_write();
        fixture.store.put(&mut write_txn, &block);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let mut delete_txn = fixture.begin_write();
        fixture.store.del(&mut delete_txn, &block.hash());
        Box::new(delete_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &block.hash()).is_none());
    }

    #[test]
    fn block_store_iterates() {
        let fixture = BlockFixture::new();
        let mut write_txn = fixture.begin_write();
        for seed in 0..3 {
            let block = unique_block(seed);
            fixture.store.put(&mut write_txn, &block);
        }
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        let count = fixture.store.iter(&read_txn).count();
        assert_eq!(count, 3);
    }

    #[test]
    fn account_store_put_get() {
        let fixture = AccountFixture::new();
        let tracker = fixture.store.track_puts();
        let account = Account::from(42);
        let info = AccountInfo::new_test_instance();

        let mut write_txn = fixture.begin_write();
        fixture.store.put(&mut write_txn, &account, &info);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &account), Some(info.clone()));
        assert_eq!(tracker.output(), vec![(account, info)]);
    }

    #[test]
    fn account_store_delete() {
        let fixture = AccountFixture::new();
        let entries = vec![
            (Account::from(1), AccountInfo::new_test_instance()),
            (Account::from(2), AccountInfo::new_test_instance()),
        ];
        fixture.insert_accounts(&entries);

        let mut write_txn = fixture.begin_write();
        fixture.store.del(&mut write_txn, &entries[0].0);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &entries[0].0).is_none());
        assert!(fixture.store.get(&read_txn, &entries[1].0).is_some());
    }

    #[test]
    fn account_store_iterates_in_order() {
        let fixture = AccountFixture::new();
        let entries = vec![
            (Account::from(1), AccountInfo::new_test_instance()),
            (Account::from(3), AccountInfo::new_test_instance()),
            (Account::from(2), AccountInfo::new_test_instance()),
        ];
        fixture.insert_accounts(&entries);

        let read_txn = fixture.begin_read();
        let accounts: Vec<_> = fixture
            .store
            .iter(&read_txn)
            .map(|(account, _)| account)
            .collect();
        assert_eq!(
            accounts,
            vec![Account::from(1), Account::from(2), Account::from(3)]
        );
    }

    #[test]
    fn account_store_iter_range() {
        let fixture = AccountFixture::new();
        let entries = vec![
            (Account::from(10), AccountInfo::new_test_instance()),
            (Account::from(20), AccountInfo::new_test_instance()),
            (Account::from(30), AccountInfo::new_test_instance()),
        ];
        fixture.insert_accounts(&entries);

        let read_txn = fixture.begin_read();
        let range = RangeBounds::new(
            Bound::Included(Account::from(15)),
            Bound::Excluded(Account::from(30)),
        );
        let accounts: Vec<_> = fixture
            .store
            .iter_range(&read_txn, range)
            .map(|(account, _)| account)
            .collect();
        assert_eq!(accounts, vec![Account::from(20)]);
    }

    #[test]
    fn account_store_count() {
        let fixture = AccountFixture::new();
        let entries = vec![
            (Account::from(1), AccountInfo::new_test_instance()),
            (Account::from(2), AccountInfo::new_test_instance()),
        ];
        fixture.insert_accounts(&entries);

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 2);
    }

    #[test]
    fn pending_store_not_found() {
        let fixture = PendingFixture::new();
        let read_txn = fixture.begin_read();
        let key = PendingKey::new_test_instance();
        assert!(fixture.store.get(&read_txn, &key).is_none());
        assert!(!fixture.store.exists(&read_txn, &key));
    }

    #[test]
    fn pending_store_put_get() {
        let fixture = PendingFixture::new();
        let key = PendingKey::new_test_instance();
        let info = PendingInfo::new_test_instance();
        let mut write_txn = fixture.begin_write();
        let tracker = fixture.store.track_puts();
        fixture.store.put(&mut write_txn, &key, &info);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &key), Some(info.clone()));
        assert_eq!(tracker.output(), vec![(key, info)]);
    }

    #[test]
    fn pending_store_delete() {
        let fixture = PendingFixture::new();
        let key = PendingKey::new_test_instance();
        let info = PendingInfo::new_test_instance();
        fixture.insert_entries(&[(key.clone(), info)]);

        let mut write_txn = fixture.begin_write();
        let tracker = fixture.store.track_deletions();
        fixture.store.del(&mut write_txn, &key);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");
        assert_eq!(tracker.output(), vec![key.clone()]);

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &key).is_none());
    }

    #[test]
    fn pending_store_iter_empty() {
        let fixture = PendingFixture::new();
        let read_txn = fixture.begin_read();
        assert!(fixture.store.iter(&read_txn).next().is_none());
    }

    #[test]
    fn pending_store_iterates() {
        let fixture = PendingFixture::new();
        let key = PendingKey::new_test_instance();
        let info = PendingInfo::new_test_instance();
        fixture.insert_entries(&[(key.clone(), info.clone())]);

        let read_txn = fixture.begin_read();
        let entries: Vec<_> = fixture.store.iter(&read_txn).collect();
        assert_eq!(entries, vec![(key, info)]);
    }

    #[test]
    fn pending_store_iter_range() {
        let fixture = PendingFixture::new();
        let k1 = PendingKey::new(Account::from(1), BlockHash::from(1));
        let k2 = PendingKey::new(Account::from(2), BlockHash::from(1));
        let k3 = PendingKey::new(Account::from(3), BlockHash::from(1));
        let info = PendingInfo::new_test_instance();
        fixture.insert_entries(&[(k1, info.clone()), (k2, info.clone()), (k3, info.clone())]);

        let read_txn = fixture.begin_read();
        let range = RangeBounds::new(
            Bound::Included(PendingKey::new(Account::from(2), BlockHash::from(0))),
            Bound::Excluded(PendingKey::new(Account::from(3), BlockHash::from(0))),
        );
        let entries: Vec<_> = fixture.store.iter_range(&read_txn, range).collect();
        assert_eq!(entries, vec![(k2, info)]);
    }

    #[test]
    fn pending_store_exists() {
        let fixture = PendingFixture::new();
        let key = PendingKey::new_test_instance();
        let info = PendingInfo::new_test_instance();
        fixture.insert_entries(&[(key.clone(), info)]);
        let read_txn = fixture.begin_read();
        assert!(fixture.store.exists(&read_txn, &key));
    }

    #[test]
    fn pending_store_any_for_account() {
        let fixture = PendingFixture::new();
        let account = Account::from(42);
        let key = PendingKey::new(account, BlockHash::from(7));
        let info = PendingInfo::new_test_instance();
        fixture.insert_entries(&[(key, info)]);

        let read_txn = fixture.begin_read();
        assert!(fixture.store.any(&read_txn, &account));
        assert!(!fixture.store.any(&read_txn, &Account::from(5)));
    }

    #[test]
    fn confirmation_store_empty() {
        let fixture = ConfirmationFixture::new();
        let read_txn = fixture.begin_read();
        let account = Account::from(1);
        assert!(fixture.store.get(&read_txn, &account).is_none());
        assert!(!fixture.store.exists(&read_txn, &account));
        assert!(fixture.store.iter(&read_txn).next().is_none());
    }

    #[test]
    fn confirmation_store_put_get() {
        let fixture = ConfirmationFixture::new();
        let account = Account::from(2);
        let info = ConfirmationHeightInfo::new(5, BlockHash::from(9));
        let mut txn = fixture.begin_write();
        fixture.store.put(&mut txn, &account, &info);
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &account), Some(info.clone()));
        assert!(fixture.store.exists(&read_txn, &account));
        assert_eq!(fixture.store.count(&read_txn), 1);
    }

    #[test]
    fn confirmation_store_delete() {
        let fixture = ConfirmationFixture::new();
        let account = Account::from(3);
        let info = ConfirmationHeightInfo::new(2, BlockHash::from(5));
        fixture.insert_entries(&[(account, info)]);

        let mut txn = fixture.begin_write();
        fixture.store.del(&mut txn, &Account::from(3));
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &Account::from(3)).is_none());
    }

    #[test]
    fn confirmation_store_iter_range() {
        let fixture = ConfirmationFixture::new();
        let entries = vec![
            (
                Account::from(1),
                ConfirmationHeightInfo::new(1, BlockHash::from(1)),
            ),
            (
                Account::from(2),
                ConfirmationHeightInfo::new(2, BlockHash::from(2)),
            ),
            (
                Account::from(3),
                ConfirmationHeightInfo::new(3, BlockHash::from(3)),
            ),
        ];
        fixture.insert_entries(&entries);

        let read_txn = fixture.begin_read();
        let range = RangeBounds::new(
            Bound::Included(Account::from(2)),
            Bound::Excluded(Account::from(3)),
        );
        let entries: Vec<_> = fixture.store.iter_range(&read_txn, range).collect();
        assert_eq!(
            entries,
            vec![(
                Account::from(2),
                ConfirmationHeightInfo::new(2, BlockHash::from(2))
            )]
        );
    }

    #[test]
    fn confirmation_store_clear() {
        let fixture = ConfirmationFixture::new();
        let entries = vec![(
            Account::from(1),
            ConfirmationHeightInfo::new(1, BlockHash::from(1)),
        )];
        fixture.insert_entries(&entries);

        let mut txn = fixture.begin_write();
        fixture.store.clear(&mut txn);
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 0);
    }

    #[test]
    fn rep_weight_count() {
        let fixture = RepWeightFixture::new();
        let entries = vec![
            (PublicKey::from(1), Amount::from(10)),
            (PublicKey::from(2), Amount::from(20)),
        ];
        fixture.insert_entries(&entries);
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 2);
    }

    #[test]
    fn rep_weight_put_get() {
        let fixture = RepWeightFixture::new();
        let mut write_txn = fixture.begin_write();
        let put_tracker = fixture.store.track_puts();
        let account = PublicKey::from(5);
        let weight = Amount::from(50);
        fixture.store.put(&mut write_txn, account, weight);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &account), Some(weight));
        assert_eq!(put_tracker.output(), vec![(account, weight)]);
    }

    #[test]
    fn rep_weight_delete() {
        let fixture = RepWeightFixture::new();
        let account = PublicKey::from(7);
        fixture.insert_entries(&[(account, Amount::from(70))]);

        let mut write_txn = fixture.begin_write();
        let delete_tracker = fixture.store.track_deletions();
        fixture.store.del(&mut write_txn, &account);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &account).is_none());
        assert_eq!(delete_tracker.output(), vec![account]);
    }

    #[test]
    fn rep_weight_iter_empty() {
        let fixture = RepWeightFixture::new();
        let read_txn = fixture.begin_read();
        assert!(fixture.store.iter(&read_txn).next().is_none());
    }

    #[test]
    fn rep_weight_iterates() {
        let fixture = RepWeightFixture::new();
        let entries = vec![
            (PublicKey::from(1), Amount::from(100)),
            (PublicKey::from(2), Amount::from(200)),
        ];
        fixture.insert_entries(&entries);

        let read_txn = fixture.begin_read();
        let items: Vec<_> = fixture.store.iter(&read_txn).collect();
        assert_eq!(items, entries);
    }

    #[test]
    fn successor_store_count() {
        let fixture = SuccessorFixture::new();
        let entries = vec![
            (BlockHash::from(1), BlockHash::from(2)),
            (BlockHash::from(3), BlockHash::from(4)),
        ];
        fixture.insert_entries(&entries);
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 2);
    }

    #[test]
    fn successor_store_put_get() {
        let fixture = SuccessorFixture::new();
        let mut txn = fixture.begin_write();
        let tracker = fixture.store.track_puts();
        let block = BlockHash::from(10);
        let successor = BlockHash::from(11);
        fixture.store.put(&mut txn, &block, &successor);
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &block), Some(successor));
        assert_eq!(tracker.output(), vec![(block, successor)]);
    }

    #[test]
    fn successor_store_delete() {
        let fixture = SuccessorFixture::new();
        let block = BlockHash::from(5);
        let successor = BlockHash::from(6);
        fixture.insert_entries(&[(block, successor)]);

        let mut txn = fixture.begin_write();
        fixture.store.del(&mut txn, &block);
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &block).is_none());
    }

    #[test]
    fn successor_store_no_entry() {
        let fixture = SuccessorFixture::new();
        let read_txn = fixture.begin_read();
        assert!(
            fixture
                .store
                .get(&read_txn, &BlockHash::from(999))
                .is_none()
        );
    }

    #[test]
    fn online_weight_empty() {
        let fixture = OnlineWeightFixture::new();
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 0);
        assert!(fixture.store.iter(&read_txn).next().is_none());
        assert!(fixture.store.iter_rev(&read_txn).next().is_none());
    }

    #[test]
    fn online_weight_put_get() {
        let fixture = OnlineWeightFixture::new();
        let mut txn = fixture.begin_write();
        fixture.store.put(&mut txn, 1, &Amount::from(100));
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        let entries: Vec<_> = fixture.store.iter(&read_txn).collect();
        assert_eq!(entries, vec![(1, Amount::from(100))]);
    }

    #[test]
    fn online_weight_iter_rev() {
        let fixture = OnlineWeightFixture::new();
        fixture.insert_entries(&[(1, Amount::from(10)), (2, Amount::from(20))]);
        let read_txn = fixture.begin_read();
        let entries: Vec<_> = fixture.store.iter_rev(&read_txn).collect();
        assert_eq!(entries, vec![(2, Amount::from(20)), (1, Amount::from(10))]);
    }

    #[test]
    fn online_weight_delete() {
        let fixture = OnlineWeightFixture::new();
        fixture.insert_entries(&[(5, Amount::from(50))]);
        let mut txn = fixture.begin_write();
        fixture.store.del(&mut txn, 5);
        Box::new(txn).commit().expect("rocksdb test commit failed");
        let read_txn = fixture.begin_read();
        assert!(fixture.store.iter(&read_txn).next().is_none());
    }

    #[test]
    fn online_weight_clear() {
        let fixture = OnlineWeightFixture::new();
        fixture.insert_entries(&[(7, Amount::from(70))]);
        let mut txn = fixture.begin_write();
        fixture.store.clear(&mut txn);
        Box::new(txn).commit().expect("rocksdb test commit failed");
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 0);
    }

    #[test]
    fn pruned_store_put_exists() {
        let fixture = PrunedFixture::new();
        let mut txn = fixture.begin_write();
        let hash = BlockHash::from(100);
        fixture.store.put(&mut txn, &hash);
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert!(fixture.store.exists(&read_txn, &hash));
    }

    #[test]
    fn pruned_store_delete() {
        let fixture = PrunedFixture::new();
        let mut txn = fixture.begin_write();
        let hash = BlockHash::from(200);
        fixture.store.put(&mut txn, &hash);
        fixture.store.del(&mut txn, &hash);
        Box::new(txn).commit().expect("rocksdb test commit failed");
        let read_txn = fixture.begin_read();
        assert!(!fixture.store.exists(&read_txn, &hash));
    }

    #[test]
    fn pruned_store_count() {
        let fixture = PrunedFixture::new();
        let mut txn = fixture.begin_write();
        fixture.store.put(&mut txn, &BlockHash::from(1));
        fixture.store.put(&mut txn, &BlockHash::from(2));
        Box::new(txn).commit().expect("rocksdb test commit failed");
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 2);
    }

    #[test]
    fn final_vote_put_and_get() {
        let fixture = FinalVoteFixture::new();
        let root = QualifiedRoot::new_test_instance();
        let hash = BlockHash::from(123);
        let mut txn = fixture.begin_write();
        assert!(fixture.store.put(&mut txn, &root, &hash));
        Box::new(txn).commit().expect("rocksdb test commit failed");
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &root), Some(hash));
    }

    #[test]
    fn final_vote_conflict_detection() {
        let fixture = FinalVoteFixture::new();
        let root = QualifiedRoot::new_test_instance();
        let mut txn = fixture.begin_write();
        assert!(fixture.store.put(&mut txn, &root, &BlockHash::from(1)));
        assert!(!fixture.store.put(&mut txn, &root, &BlockHash::from(2)));
    }

    #[test]
    fn final_vote_delete_and_clear() {
        let fixture = FinalVoteFixture::new();
        let root = QualifiedRoot::new_test_instance();
        let hash = BlockHash::from(42);

        let mut insert_txn = fixture.begin_write();
        fixture.store.put(&mut insert_txn, &root, &hash);
        Box::new(insert_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let mut delete_txn = fixture.begin_write();
        fixture.store.del(&mut delete_txn, &root);
        Box::new(delete_txn)
            .commit()
            .expect("rocksdb test commit failed");
        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &root).is_none());

        let mut reinsertion_txn = fixture.begin_write();
        fixture.store.put(&mut reinsertion_txn, &root, &hash);
        Box::new(reinsertion_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let mut clear_txn = fixture.begin_write();
        fixture.store.clear(&mut clear_txn);
        Box::new(clear_txn)
            .commit()
            .expect("rocksdb test commit failed");
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 0);
    }

    #[test]
    fn peer_store_put_tracks() {
        let fixture = PeerFixture::new();
        let mut txn = fixture.begin_write();
        let tracker = fixture.store.track_puts();
        let addr = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 7000, 0, 0);
        let time = UNIX_EPOCH + Duration::from_secs(10);

        fixture.store.put(&mut txn, addr, time);
        assert_eq!(tracker.output(), vec![(addr, time)]);
    }

    #[test]
    fn peer_store_delete_tracks() {
        let fixture = PeerFixture::new();
        let mut txn = fixture.begin_write();
        let tracker = fixture.store.track_deletions();
        let addr = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 7001, 0, 0);

        fixture.store.del(&mut txn, addr);
        assert_eq!(tracker.output(), vec![addr]);
    }

    #[test]
    fn peer_store_exists_and_iterates() {
        let fixture = PeerFixture::new();
        let mut txn = fixture.begin_write();
        let addr1 = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 7100, 0, 0);
        let addr2 = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 7101, 0, 0);
        fixture
            .store
            .put(&mut txn, addr1, UNIX_EPOCH + Duration::from_secs(1));
        fixture
            .store
            .put(&mut txn, addr2, UNIX_EPOCH + Duration::from_secs(2));
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert!(fixture.store.exists(&read_txn, addr1));
        let peers: Vec<_> = fixture.store.iter(&read_txn).collect();
        assert_eq!(peers.len(), 2);
    }

    #[test]
    fn peer_store_clear() {
        let fixture = PeerFixture::new();
        let mut txn = fixture.begin_write();
        let addr = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 7200, 0, 0);
        fixture
            .store
            .put(&mut txn, addr, UNIX_EPOCH + Duration::from_secs(3));
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let mut clear_txn = fixture.begin_write();
        fixture.store.clear(&mut clear_txn);
        Box::new(clear_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 0);
    }

    #[test]
    fn version_store_initially_empty() {
        let fixture = VersionFixture::new();
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn), None);
    }

    #[test]
    fn version_store_put_and_get() {
        let fixture = VersionFixture::new();
        let mut write_txn = fixture.begin_write();
        fixture.store.put(&mut write_txn, 42);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn), Some(42));
    }

    fn unique_block(seed: u8) -> SavedBlock {
        let key = PrivateKey::from(u64::from(seed) + 42);
        let block = Block::new_test_instance_with_key(key);
        SavedBlock::new_test_instance_with(block)
    }
}
