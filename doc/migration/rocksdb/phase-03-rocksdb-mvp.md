### Phase 03: RocksDB MVP (VersionStore first)

Objective: Introduce a new `store_rocksdb` crate with the minimal implementation to pass `VersionStore` tests with transactional semantics (writes staged; visible only after commit).
ALWAYS RUN cargo test on the WHOLE codebase! tests are fast

Steps:

1. Create crate `store_rocksdb`
   - `cargo new store_rocksdb --lib`
   - Add dependencies: `rocksdb`, `store_api`

2. Implement minimal provider
   - Types: `RocksProvider`, `RocksReadTxn`, `RocksWriteTxn`, `RocksVersionStore`
   - Transaction semantics: use `WriteBatch` to stage writes; `commit()` applies batch atomically and flushes
   - Reads: define consistent read semantics (snapshot or documented direct reads)

3. Unit tests
   - Open temp dir; verify set is not visible before commit; visible after commit; persists across reopen

Commands:
 - `cargo test -p store_rocksdb`

Exit criteria:
 - RocksDB tests pass independently

Rollback:
 - Remove crate from workspace
