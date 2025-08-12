### Phase 05: Switch & Cleanup

Objective: Make provider selection configurable and remove direct LMDB coupling from `ledger`.
ALWAYS RUN cargo test on the WHOLE codebase! tests are fast


Steps:
1. `Ledger` holds `Box<dyn store_api::StoreProvider>` or is generic over provider
2. `LedgerBuilder` selects provider (LMDB default; RocksDB via feature/config)
3. Remove direct imports of `rsnano_nullable_lmdb` from `ledger`
4. Update `store_vendor()` to reflect active backend
5. Documentation update

Commands:
 - `cargo test -p rsnano_ledger`
 - `cargo test`
 - With Rocks: `cargo test --features rocksdb_stores`

Exit criteria:
 - All tests pass with LMDB
 - Feature-gated RocksDB path compiles and passes its slice of tests

Rollback:
 - Flip provider selection back to LMDB and reintroduce minimal LMDB types behind a compat layer
