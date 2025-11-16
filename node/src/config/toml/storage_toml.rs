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
    config.ledger_store_config = LedgerStoreConfig {
        sync,
        backend: LedgerBackend::RocksDb(rocks_config),
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
        }
    }
}

impl From<&RocksDbConfig> for RocksDbToml {
    fn from(cfg: &RocksDbConfig) -> Self {
        Self {
            max_open_files: cfg.max_open_files,
        }
    }
}
