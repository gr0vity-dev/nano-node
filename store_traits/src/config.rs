use serde::{Deserialize, Serialize};

#[derive(PartialEq, Eq, Clone, Copy, Debug, Serialize, Deserialize)]
pub enum StoreSyncStrategy {
    /// Always flush to disk on commit. This is default.
    Always,
    /// Do not flush meta data eagerly. This may cause loss of transactions, but maintains integrity.
    NosyncSafe,
    /// Let the OS decide when to flush to disk. Guarantees depend on filesystem ordering.
    NosyncUnsafe,
    /// Use a writable memory map and let the OS flush asynchronously.
    NosyncUnsafeLargeMemory,
    /// Never sync.
    NosyncUnsafeWriteMap,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerStoreConfig {
    pub sync: StoreSyncStrategy,
    pub backend: LedgerBackend,
}

impl Default for LedgerStoreConfig {
    fn default() -> Self {
        Self {
            sync: StoreSyncStrategy::Always,
            backend: LedgerBackend::Lmdb(LmdbConfig::default()),
        }
    }
}

impl LedgerStoreConfig {
    pub fn new(backend: LedgerBackend) -> Self {
        Self {
            backend,
            ..Default::default()
        }
    }

    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LedgerBackend {
    Lmdb(LmdbConfig),
    RocksDb(RocksDbConfig),
}

impl LedgerBackend {
    pub fn name(&self) -> &'static str {
        match self {
            LedgerBackend::Lmdb(_) => "lmdb",
            LedgerBackend::RocksDb(_) => "rocksdb",
        }
    }

    pub fn as_lmdb(&self) -> &LmdbConfig {
        match self {
            LedgerBackend::Lmdb(cfg) => cfg,
            _ => panic!("not an LMDB backend"),
        }
    }

    pub fn as_lmdb_mut(&mut self) -> &mut LmdbConfig {
        match self {
            LedgerBackend::Lmdb(cfg) => cfg,
            _ => panic!("not an LMDB backend"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LmdbConfig {
    pub max_databases: u32,
    pub map_size: usize,
    pub mem_init: bool,
}

impl Default for LmdbConfig {
    fn default() -> Self {
        Self {
            max_databases: 128,
            map_size: 256 * 1024 * 1024 * 1024,
            mem_init: false,
        }
    }
}

impl LmdbConfig {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RocksDbConfig {
    pub max_open_files: Option<i32>,
}

impl Default for RocksDbConfig {
    fn default() -> Self {
        Self {
            max_open_files: None,
        }
    }
}
