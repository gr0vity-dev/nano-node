### Phase 02: LMDB Adapter (Minimal Slice)

Objective: Implement `store_api` traits for LMDB with the smallest surface (VersionStore + txn).

Steps:

1. Add dependency
   - In `store_lmdb/Cargo.toml`: `store_api = { path = "../store_api" }`

2. Implement adapters
   - New file `store_lmdb/src/adapter.rs`
   - Implement:
     - `impl store_api::TransactionLike for rsnano_nullable_lmdb::ReadTransaction/WriteTransaction`
     - `impl store_api::ReadTxnLike for ReadTransaction`
     - `impl store_api::WriteTxnLike for WriteTransaction`
     - `impl store_api::VersionStore for LmdbVersionStore`
     - `impl store_api::StoreProvider for LmdbStore` (only wire version + begin_{read,write} + refresh)

3. Add a unit test in `store_lmdb`
   - Create null env, new store, write version via trait, read it back

Commands:
 - `cargo test -p rsnano_store_lmdb`

Exit criteria:
 - Tests pass; no changes to other crates

Rollback:
 - Remove `adapter.rs` and dependency from `Cargo.toml`
