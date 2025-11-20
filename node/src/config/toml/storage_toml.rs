use crate::config::NodeConfig;
use serde::{Deserialize, Serialize};
use store_traits::config::{LedgerBackend, LedgerStoreConfig, LmdbConfig, RocksDbConfig};

use super::LmdbToml;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StorageToml {
    pub backend: Option<String>,
    pub lmdb: Option<LmdbToml>,
    pub rocksdb: Option<RocksDbToml>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RocksDbToml {
    pub max_open_files: Option<i32>,
    pub enable_pipelined_write: Option<bool>,
    pub allow_concurrent_memtable_write: Option<bool>,
    pub write_buffer_size: Option<u64>,
    pub max_write_buffer_number: Option<i32>,
    pub min_write_buffer_number_to_merge: Option<i32>,
    pub max_background_jobs: Option<i32>,
}

impl StorageToml {
    pub fn apply(&self, config: &mut NodeConfig) {
        if let Some(choice) = self.desired_backend_choice() {
            match choice {
                StorageBackendChoice::RocksDb => {
                    let rocks_config = self
                        .rocksdb
                        .as_ref()
                        .map(RocksDbConfig::from)
                        .unwrap_or_else(|| {
                            config
                                .ledger_store_config
                                .backend
                                .as_rocksdb()
                                .cloned()
                                .unwrap_or_default()
                        });
                    apply_rocksdb_config(config, rocks_config);
                }
                StorageBackendChoice::Lmdb => {
                    if let Some(lmdb) = &self.lmdb {
                        config.ledger_store_config = lmdb.into();
                    } else {
                        let sync = config.ledger_store_config.sync;
                        config.ledger_store_config = LedgerStoreConfig {
                            sync,
                            backend: LedgerBackend::Lmdb(LmdbConfig::default()),
                        };
                    }
                }
            }
        } else {
            if let Some(lmdb) = &self.lmdb {
                if matches!(config.ledger_store_config.backend, LedgerBackend::Lmdb(_)) {
                    config.ledger_store_config = lmdb.into();
                }
            }
            if let Some(rocksdb) = &self.rocksdb {
                if matches!(
                    config.ledger_store_config.backend,
                    LedgerBackend::RocksDb(_)
                ) {
                    apply_rocksdb_config(config, rocksdb.into());
                }
            }
        }
    }

    fn desired_backend_choice(&self) -> Option<StorageBackendChoice> {
        if let Some(backend) = &self.backend {
            let normalized = backend.to_ascii_lowercase();
            match normalized.as_str() {
                "rocksdb" => Some(StorageBackendChoice::RocksDb),
                "lmdb" => Some(StorageBackendChoice::Lmdb),
                other => panic!("Unsupported storage backend '{other}'"),
            }
        } else {
            None
        }
    }
}

fn apply_rocksdb_config(config: &mut NodeConfig, rocks_config: RocksDbConfig) {
    let sync = config.ledger_store_config.sync;
    let backend_cfg = if config.rocksdb_optimizations_enabled {
        rocks_config
    } else {
        RocksDbConfig {
            max_open_files: rocks_config.max_open_files,
            enable_pipelined_write: false,
            allow_concurrent_memtable_write: false,
            write_buffer_size: rocks_config.write_buffer_size,
            max_write_buffer_number: rocks_config.max_write_buffer_number,
            min_write_buffer_number_to_merge: rocks_config.min_write_buffer_number_to_merge,
            max_background_jobs: rocks_config.max_background_jobs,
        }
    };
    config.ledger_store_config = LedgerStoreConfig {
        sync,
        backend: LedgerBackend::RocksDb(backend_cfg),
    };
}

enum StorageBackendChoice {
    RocksDb,
    Lmdb,
}

impl From<&LedgerStoreConfig> for StorageToml {
    fn from(config: &LedgerStoreConfig) -> Self {
        let backend_name = config.backend_name().to_string();
        let mut storage = StorageToml {
            backend: Some(backend_name),
            ..Default::default()
        };

        match &config.backend {
            LedgerBackend::Lmdb(_) => storage.lmdb = Some(config.into()),
            LedgerBackend::RocksDb(rocks) => storage.rocksdb = Some(rocks.into()),
        }

        storage
    }
}

impl From<&RocksDbToml> for RocksDbConfig {
    fn from(toml: &RocksDbToml) -> Self {
        RocksDbConfig {
            max_open_files: toml.max_open_files,
            enable_pipelined_write: toml.enable_pipelined_write.unwrap_or(false),
            allow_concurrent_memtable_write: toml.allow_concurrent_memtable_write.unwrap_or(false),
            write_buffer_size: toml.write_buffer_size,
            max_write_buffer_number: toml.max_write_buffer_number,
            min_write_buffer_number_to_merge: toml.min_write_buffer_number_to_merge,
            max_background_jobs: toml.max_background_jobs,
        }
    }
}

impl From<&RocksDbConfig> for RocksDbToml {
    fn from(cfg: &RocksDbConfig) -> Self {
        Self {
            max_open_files: cfg.max_open_files,
            enable_pipelined_write: Some(cfg.enable_pipelined_write),
            allow_concurrent_memtable_write: Some(cfg.allow_concurrent_memtable_write),
            write_buffer_size: cfg.write_buffer_size,
            max_write_buffer_number: cfg.max_write_buffer_number,
            min_write_buffer_number_to_merge: cfg.min_write_buffer_number_to_merge,
            max_background_jobs: cfg.max_background_jobs,
        }
    }
}
