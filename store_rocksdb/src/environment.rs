use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use parking_lot::RwLock;
use rocksdb::{
    BoundColumnFamily, ColumnFamilyDescriptor, Error as RocksError, IteratorMode, MultiThreaded,
    OptimisticTransactionDB, Options, SnapshotWithThreadMode,
};
use store_traits::config::RocksDbConfig;
use store_traits::environment::{
    StoreEnvironment, StoreEnvironmentFactory, StoreEnvironmentOptions,
};
use store_traits::types::{
    StoreDatabase, StoreEnvironmentFlags, StoreError, StoreErrorKind, StoreResult,
};

use crate::{
    transaction::{RocksdbReadTxn, RocksdbWriteTxn},
    write_queue::WriteQueue,
};
use store_traits::ledger::{WriteStrategy, WriterType};

pub struct RocksdbStoreEnvironment {
    inner: Arc<RocksDbInner>,
    _temp_dir: Option<tempfile::TempDir>,
}

impl RocksdbStoreEnvironment {
    pub fn open(
        path: PathBuf,
        _flags: StoreEnvironmentFlags,
        temp_dir: Option<tempfile::TempDir>,
        config: Option<&RocksDbConfig>,
    ) -> Result<Self> {
        let inner = RocksDbInner::open(&path, config)?;
        Ok(Self {
            inner: Arc::new(inner),
            _temp_dir: temp_dir,
        })
    }

    pub(crate) fn inner(&self) -> Arc<RocksDbInner> {
        Arc::clone(&self.inner)
    }

    pub fn open_db(&self, name: Option<&str>) -> StoreResult<StoreDatabase> {
        self.inner.open_database(name)
    }

    pub fn sync(&self) -> StoreResult<()> {
        self.inner.flush_wal()
    }

    pub fn applied_config(&self) -> Option<RocksDbConfig> {
        self.inner.config.clone()
    }
}

impl StoreEnvironment for RocksdbStoreEnvironment {
    type ReadTxn<'env> = RocksdbReadTxn;
    type WriteTxn<'env> = RocksdbWriteTxn;

    fn begin_read(&self) -> Self::ReadTxn<'_> {
        RocksdbReadTxn::new(&self.inner)
    }

    fn begin_write(&self) -> Self::WriteTxn<'_> {
        RocksdbWriteTxn::new(&self.inner, WriterType::Generic, WriteStrategy::Optimistic)
    }

    fn open_db(&self, name: Option<&str>) -> StoreResult<StoreDatabase> {
        self.inner.open_database(name)
    }

    fn sync(&self) -> StoreResult<()> {
        self.inner.flush_wal()
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

pub(crate) struct RocksDbInner {
    pub(crate) db: RocksDb,
    registry: RwLock<CfRegistry>,
    config: Option<RocksDbConfig>,
    write_queue: WriteQueue,
}

pub(crate) type RocksDb = OptimisticTransactionDB<MultiThreaded>;
pub(crate) type RocksDbSnapshot<'a> = SnapshotWithThreadMode<'a, RocksDb>;

pub(crate) const BLOCK_INDEX_CF_NAME: &str = "rocksdb_block_index";
pub(crate) const BLOCK_DATA_CF_NAME: &str = "rocksdb_block_data";
pub(crate) const ACCOUNTS_CF_NAME: &str = "rocksdb_accounts";
pub(crate) const PENDING_CF_NAME: &str = "rocksdb_pending";
pub(crate) const CONF_HEIGHT_CF_NAME: &str = "rocksdb_confirmation_height";
pub(crate) const REP_WEIGHT_CF_NAME: &str = "rocksdb_rep_weights";
pub(crate) const SUCCESSOR_CF_NAME: &str = "rocksdb_successors";
pub(crate) const ONLINE_WEIGHT_CF_NAME: &str = "rocksdb_online_weight";
pub(crate) const PRUNED_CF_NAME: &str = "rocksdb_pruned";
pub(crate) const FINAL_VOTE_CF_NAME: &str = "rocksdb_final_votes";
pub(crate) const PEERS_CF_NAME: &str = "rocksdb_peers";
pub(crate) const VERSION_CF_NAME: &str = "rocksdb_version";

impl RocksDbInner {
    fn open(path: &Path, config: Option<&RocksDbConfig>) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);
        if let Some(cfg) = config {
            if let Some(max_open_files) = cfg.max_open_files {
                options.set_max_open_files(max_open_files);
            }
            apply_tuning_options(&mut options, cfg);
        }

        let cf_names = if path.exists() {
            RocksDb::list_cf(&options, path).unwrap_or_default()
        } else {
            Vec::new()
        };

        let db = if cf_names.is_empty() {
            let descriptor = ColumnFamilyDescriptor::new(
                rocksdb::DEFAULT_COLUMN_FAMILY_NAME,
                column_family_options(config),
            );
            RocksDb::open_cf_descriptors(&options, path, vec![descriptor])?
        } else {
            let descriptors = cf_names
                .iter()
                .map(|name| ColumnFamilyDescriptor::new(name, column_family_options(config)))
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
            config: config.cloned(),
            write_queue: WriteQueue::new(),
        })
    }

    pub(crate) fn snapshot(&self) -> RocksDbSnapshot<'_> {
        self.db.snapshot()
    }

    pub(crate) fn flush_wal(&self) -> StoreResult<()> {
        self.db.flush_wal(true).map_err(store_error_from_rocksdb)
    }

    pub(crate) fn open_database(&self, name: Option<&str>) -> StoreResult<StoreDatabase> {
        let name = name.unwrap_or(rocksdb::DEFAULT_COLUMN_FAMILY_NAME);
        if let Some(handle) = self.registry.read().handle_for_name(name) {
            return Ok(handle);
        }

        let mut cf_options = column_family_options(self.config.as_ref());
        cf_options.create_if_missing(true);
        self.db
            .create_cf(name, &cf_options)
            .map_err(store_error_from_rocksdb)?;
        self.db
            .cf_handle(name)
            .ok_or_else(|| StoreError::backend(format!("missing column family {name}")))?;
        Ok(self.registry.write().insert_existing(name))
    }

    pub(crate) fn cf_handle(
        &self,
        database: StoreDatabase,
    ) -> StoreResult<Arc<BoundColumnFamily<'_>>> {
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

    pub(crate) fn count_snapshot_entries(
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

    pub(crate) fn delete_cf(&self, database: StoreDatabase) -> StoreResult<()> {
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

    pub(crate) fn write_queue(&self) -> &WriteQueue {
        &self.write_queue
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

pub(crate) fn store_error_from_rocksdb(err: RocksError) -> StoreError {
    let kind = match err.kind() {
        rocksdb::ErrorKind::NotFound => StoreErrorKind::NotFound,
        rocksdb::ErrorKind::InvalidArgument => StoreErrorKind::InvalidArgument,
        rocksdb::ErrorKind::Corruption => StoreErrorKind::Corruption,
        rocksdb::ErrorKind::Busy | rocksdb::ErrorKind::TryAgain => StoreErrorKind::Conflict,
        _ => StoreErrorKind::Backend,
    };
    StoreError::new(kind, err.to_string())
}

fn column_family_options(config: Option<&RocksDbConfig>) -> Options {
    let mut options = Options::default();
    if let Some(cfg) = config {
        apply_tuning_options(&mut options, cfg);
    }
    options
}

fn apply_tuning_options(options: &mut Options, config: &RocksDbConfig) {
    if config.enable_pipelined_write {
        options.set_enable_pipelined_write(true);
    }
    if config.allow_concurrent_memtable_write {
        options.set_allow_concurrent_memtable_write(true);
    }
    if let Some(size) = config.write_buffer_size {
        options.set_write_buffer_size(size as usize);
    }
    if let Some(count) = config.max_write_buffer_number {
        options.set_max_write_buffer_number(count);
    }
    if let Some(count) = config.min_write_buffer_number_to_merge {
        options.set_min_write_buffer_number_to_merge(count);
    }
    if let Some(jobs) = config.max_background_jobs {
        options.set_max_background_jobs(jobs);
    }
}
