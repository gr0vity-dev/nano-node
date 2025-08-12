### Phase 03: RocksDB MVP (VersionStore first)

Objective: Introduce a new `store_rocksdb` crate with the minimal implementation to pass `VersionStore` tests.
ALWAYS RUN cargo test on the WHOLE codebase! tests are fast

Steps:

1. Create crate `store_rocksdb`
   - `cargo new store_rocksdb --lib`
   - Add dependencies: `rocksdb`, `store_api`

2. Implement minimal provider
   - Types: `RocksProvider`, `RocksReadTxn`, `RocksWriteTxn`, `RocksVersionStore`
   - `VersionStore` uses a dedicated column family or a fixed key in default CF

3. Unit tests
   - Open temp dir, set/get version; ensure persistence across reopen

Commands:
 - `cargo test -p store_rocksdb`

Exit criteria:
 - RocksDB tests pass independently

Rollback:
 - Remove crate from workspace
