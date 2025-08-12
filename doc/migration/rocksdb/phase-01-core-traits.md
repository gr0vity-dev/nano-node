### Phase 01: Core Traits (Minimal API)

Objective: Introduce a minimal storage abstraction crate without changing runtime behavior.

Scope per step: ≤ 50 LOC edits, compile and test after each.

Steps:

1. Create crate `store_api`
   - Files:
     - `store_api/Cargo.toml`
     - `store_api/src/lib.rs`
   - Contents (sketch):
     - `pub trait TransactionLike { fn is_refresh_needed(&self) -> bool; }`
     - `pub trait ReadTxnLike: TransactionLike {}`
     - `pub trait WriteTxnLike: TransactionLike { fn commit(&mut self); }`
     - `pub trait VersionStore { fn get(&self, r: &impl ReadTxnLike) -> Option<i32>; fn set(&self, w: &mut impl WriteTxnLike, v: i32); }`
     - `pub trait StoreProvider { type R: ReadTxnLike; type W: WriteTxnLike; fn begin_read(&self) -> Self::R; fn begin_write(&self) -> Self::W; fn refresh(&self, w: Self::W) -> Self::W; fn version(&self) -> &dyn VersionStore; }`
   - Commands:
     - `cargo new store_api --lib`
     - `cargo test -p store_api`

2. Wire LMDB adapters (standalone)
   - Files:
     - `store_lmdb/Cargo.toml` add dep: `store_api = { path = "../store_api" }`
     - `store_lmdb/src/adapter.rs` implement the traits for LMDB types
   - Keep LOC small by only implementing `VersionStore` and txn traits
   - Commands:
     - `cargo test -p rsnano_store_lmdb`

3. No changes to `ledger` yet
   - Ensure repo builds: `cargo build`

Exit criteria:
 - `store_api` compiles
 - `rsnano_store_lmdb` compiles with adapter
 - No behavior change in runtime crates

Rollback:
 - Remove `store_api` from workspace and revert `store_lmdb` dependency and adapter file
