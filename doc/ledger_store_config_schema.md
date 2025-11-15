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
```

- `sync` is the single cross-backend knob and matches the variants defined in
  `StoreSyncStrategy`.
- `backend` selects which adapter to instantiate and carries the strongly typed
  options for that backend.
- `LmdbConfig` exposes the knobs that used to live on `LedgerStoreConfig`
  directly, so logic/application crates never import LMDB-specific types.

## TOML Example

```toml
[node.storage]
backend = { kind = "lmdb" }
sync = "nosync"

[node.storage.lmdb]
map_size_gb = 128
mem_init = true
```

The application layer forwards the selected backend and its typed config
directly into the `LedgerStoreFactory`. Tests should exercise real backends
through these APIs instead of poking at JSON blobs.
