# LedgerStoreConfig Schema

This document captures the target schema for `LedgerStoreConfig` before the backend
toggle becomes fully dynamic. The schema deliberately separates **common** options
from **backend-specific** blobs so application and logic crates do not depend on
LMDB details.

## Struct Layout

```rust
pub struct LedgerStoreConfig {
    pub sync: StoreSyncStrategy,
    pub backend: LedgerBackend,
}

pub enum LedgerBackend {
    Lmdb(LmdbConfig),
    RocksDb(RocksDbConfig),
}

pub struct LmdbConfig {
    pub max_databases: u32,
    pub map_size: usize,
    pub mem_init: bool,
}

pub struct RocksDbConfig {
    pub max_open_files: Option<i32>,
    pub enable_pipelined_write: bool,
    pub allow_concurrent_memtable_write: bool,
    pub write_buffer_size: Option<u64>,
    pub max_write_buffer_number: Option<i32>,
    pub min_write_buffer_number_to_merge: Option<i32>,
    pub max_background_jobs: Option<i32>,
    pub enable_iterator_stats: bool,
}
```

- `sync` is the single cross-backend knob and matches the variants defined in
  `StoreSyncStrategy`.
- `backend` selects which adapter to instantiate and carries the strongly typed
  options for that backend.
- `LmdbConfig` exposes the knobs that used to live on `LedgerStoreConfig`
  directly, so logic/application crates never import LMDB-specific types.
- `RocksDbConfig` wires the RocksDB tuning knobs that the adapter currently
  supports. Fields are optional unless otherwise noted so operators can opt
  into more aggressive configurations without recompiling.

## TOML Example

```toml
[node.storage]
backend = { kind = "lmdb" }
sync = "nosync"

[node.storage.backend]
kind = "rocksdb"

[node.storage.rocksdb]
max_open_files = 2000
```

The application layer forwards the selected backend and its typed config
directly into the `LedgerStoreFactory`. Tests should exercise real backends
through these APIs instead of poking at JSON blobs. RocksDB support is under
active development; until the adapter lands, selecting `rocksdb` will return an
explicit error.
