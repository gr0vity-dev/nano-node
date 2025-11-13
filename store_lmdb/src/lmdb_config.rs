use std::path::PathBuf;

use rsnano_nullable_lmdb::{EnvironmentFlags, EnvironmentOptions};
use store_traits::config::StoreSyncStrategy;

pub use store_traits::config::{
    LedgerStoreConfig as LmdbConfig, StoreSyncStrategy as SyncStrategy,
};

pub fn get_lmdb_flags(config: &LmdbConfig) -> EnvironmentFlags {
    // It seems if there's ever more threads than mdb_env_set_maxreaders has read slots available, we get failures on transaction creation unless MDB_NOTLS is specified
    // This can happen if something like 256 io_threads are specified in the node config
    // MDB_NORDAHEAD will allow platforms that support it to load the DB in memory as needed.
    // MDB_NOMEMINIT prevents zeroing malloc'ed pages. Can provide improvement for non-sensitive data but may make memory checkers noisy (e.g valgrind).
    let mut flags = EnvironmentFlags::NO_SUB_DIR | EnvironmentFlags::NO_TLS;

    match config.sync {
        StoreSyncStrategy::NosyncSafe => {
            flags |= EnvironmentFlags::NO_META_SYNC;
        }
        StoreSyncStrategy::NosyncUnsafe => {
            flags |= EnvironmentFlags::NO_SYNC | EnvironmentFlags::NO_META_SYNC;
        }
        StoreSyncStrategy::NosyncUnsafeLargeMemory => {
            flags |= EnvironmentFlags::NO_SYNC
                | EnvironmentFlags::WRITE_MAP
                | EnvironmentFlags::MAP_ASYNC;
        }
        StoreSyncStrategy::NosyncUnsafeWriteMap => {
            flags |= EnvironmentFlags::NO_SYNC | EnvironmentFlags::WRITE_MAP;
        }
        StoreSyncStrategy::Always => {}
    }

    if !config.mem_init {
        flags |= EnvironmentFlags::NO_MEM_INIT;
    }
    flags
}

pub fn default_ledger_lmdb_options(path: impl Into<PathBuf>) -> EnvironmentOptions {
    EnvironmentOptions {
        max_dbs: 128,
        map_size: 256 * 1024 * 1024 * 1024,
        flags: EnvironmentFlags::NO_SUB_DIR
            | EnvironmentFlags::NO_TLS
            | EnvironmentFlags::NO_MEM_INIT,
        path: path.into(),
    }
}
