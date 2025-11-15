use serde::{Deserialize, Serialize};
use store_traits::config::{LedgerBackend, LedgerStoreConfig, LmdbConfig, StoreSyncStrategy};

#[derive(Deserialize, Serialize)]
pub struct LmdbToml {
    pub map_size: Option<usize>,
    pub max_databases: Option<u32>,
    pub sync: Option<String>,
}

impl Default for LmdbToml {
    fn default() -> Self {
        let config = LedgerStoreConfig::default();
        (&config).into()
    }
}

impl From<&LmdbToml> for LedgerStoreConfig {
    fn from(toml: &LmdbToml) -> Self {
        let mut sync = StoreSyncStrategy::Always;
        if let Some(sync_str) = &toml.sync {
            sync = match sync_str.as_str() {
                "always" => StoreSyncStrategy::Always,
                "nosync_safe" => StoreSyncStrategy::NosyncSafe,
                "nosync_unsafe" => StoreSyncStrategy::NosyncUnsafe,
                "nosync_unsafe_large_memory" => StoreSyncStrategy::NosyncUnsafeLargeMemory,
                "nosync_unsafe_write_map" => StoreSyncStrategy::NosyncUnsafeWriteMap,
                other => panic!("Invalid sync value: {other}"),
            };
        }

        let mut backend = LmdbConfig::default();
        if let Some(max_databases) = toml.max_databases {
            backend.max_databases = max_databases;
        }
        if let Some(map_size) = toml.map_size {
            backend.map_size = map_size;
        }
        LedgerStoreConfig {
            sync,
            backend: LedgerBackend::Lmdb(backend),
        }
    }
}

impl From<&LedgerStoreConfig> for LmdbToml {
    fn from(config: &LedgerStoreConfig) -> Self {
        let backend = match &config.backend {
            LedgerBackend::Lmdb(cfg) => cfg,
            _ => panic!("LMDB TOML conversion requires LMDB backend"),
        };
        Self {
            sync: Some(match config.sync {
                StoreSyncStrategy::Always => "always".to_string(),
                StoreSyncStrategy::NosyncSafe => "nosync_safe".to_string(),
                StoreSyncStrategy::NosyncUnsafe => "nosync_unsafe".to_string(),
                StoreSyncStrategy::NosyncUnsafeLargeMemory => {
                    "nosync_unsafe_large_memory".to_string()
                }
                StoreSyncStrategy::NosyncUnsafeWriteMap => "nosync_unsafe_write_map".to_string(),
            }),
            max_databases: Some(backend.max_databases),
            map_size: Some(backend.map_size),
        }
    }
}
