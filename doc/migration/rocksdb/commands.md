### Commands Cheat Sheet

- **Repo-wide build and test**
  - `cargo build`
  - `cargo test`

- **Crate-specific builds**
  - Ledger: `cargo test -p rsnano_ledger`
  - Store LMDB: `cargo test -p rsnano_store_lmdb`
  - Node: `cargo test -p rsnano_node`

- **Focus one crate** (faster inner loop)
  - `cargo test -p <crate> -- --nocapture`

- **Suggested loop per micro-step**
  1. Make ≤ 50 LOC edits
  2. `cargo test -p <impacted-crate>`
  3. `cargo test` (repo-wide)
  4. Commit with scope, e.g. `feat(store): add VersionStore trait`

- **Optional features** (to be added later)
  - RocksDB stores: `--features rocksdb_stores`
  - Mixed backends: `--features mixed_store_backends`
