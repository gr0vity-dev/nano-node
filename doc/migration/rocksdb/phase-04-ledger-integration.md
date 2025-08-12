### Phase 04: Ledger Integration (Micro-swaps)

Objective: Transition `ledger` off LMDB types in tiny increments by swapping individual callsites to trait-based access. Keep LMDB as the concrete provider until the minimal surface is complete. Do not mix core stores across backends in production.
ALWAYS RUN cargo test on the WHOLE codebase! tests are fast


Order of swaps (each is a separate micro-step):
1. `Ledger::version()` → use `VersionStore` trait (LMDB provider)
2. Rep weights initialization → accept trait-backed store in `RepWeightsUpdater` (LMDB provider)
3. Pruning paths → `PrunedStore` + minimal `BlockStore` trait (exists/get/del) used only in pruning (LMDB provider)
4. Cache initializations (account/confirmed counts) → trait iteration wrappers for `AccountStore`/`ConfirmationHeightStore`
5. Remaining sub-stores: `ConfirmationHeightStore`, `AccountStore`, `PendingStore`, `SuccessorStore`, `PeerStore`

Per micro-step checklist:
 - Add/extend trait in `store_api`
 - Implement LMDB adapter + unit tests
  - Implement RocksDB adapter + unit tests in parallel (not wired into runtime yet)
 - Switch exactly one method in `ledger` to use the trait
 - Run: `cargo test -p rsnano_ledger` and then `cargo test`

Exit criteria:
 - All ledger interactions go through `store_api` traits

Rollback:
 - Revert the last method change in `ledger` and keep adapters/tests
