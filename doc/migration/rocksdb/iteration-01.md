### Iteration 01 — Minimal API + LMDB VersionStore adapter

ALWAYS RUN cargo test on the WHOLE codebase! tests are fast

Constraints:
- Keep edits ≤ 50 LOC per file
- Do NEVER modify tests
- ALWAYS RUN cargo test on the WHOLE codebase! tests are fast

Current state:
- `store_api` crate created with minimal traits: `TransactionLike`, `ReadTxnLike`, `WriteTxnLike`, `VersionStore`, `StoreProvider`

Deliverables for this iteration:
1) LMDB adapters (VersionStore + txn only)
   - Modify `store_lmdb/Cargo.toml` to depend on `store_api`
   - Add `store_lmdb/src/adapter.rs` implementing:
     - `impl store_api::TransactionLike for rsnano_nullable_lmdb::{ReadTransaction, WriteTransaction}`
     - `impl store_api::ReadTxnLike for ReadTransaction`
     - `impl store_api::WriteTxnLike for WriteTransaction` (delegating `commit()`)
     - `impl store_api::VersionStore for LmdbVersionStore` (bridge to existing get/set)
     - `impl store_api::StoreProvider for LmdbStore` (begin_read, begin_write, refresh, version)

2) Unit test in `store_lmdb`
   - Create a null env, `LmdbStore`, write+read version through trait

Edit budget guidance:
- `store_lmdb/Cargo.toml`: +1 dependency line
- `store_lmdb/src/adapter.rs`: ≤ 50 LOC
- `store_lmdb/src/lib.rs`: add `mod adapter;` (≤ 2 LOC)
- `store_lmdb/src/adapter_tests.rs`: ≤ 40 LOC (or place test under `adapter.rs` with `#[cfg(test)]`)

Commands:
- Focused:
  - `cargo test -p rsnano_store_lmdb`
- Repo-wide:
  - `cargo test`

Acceptance criteria:
- `rsnano_store_lmdb` adapter test passes
- No behavior changes in `ledger` or other crates
- Repo continues to build; tests remain green (modulo known flaky http_client tests)

Rollback:
- Remove `store_api` dependency from `store_lmdb/Cargo.toml`
- Remove `adapter.rs` and adapter tests
