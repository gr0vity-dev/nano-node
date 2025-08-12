### Phase 04: Ledger Integration (Micro-swaps)

Objective: Transition `ledger` off LMDB types in tiny increments by swapping individual callsites to trait-based access.
ALWAYS RUN cargo test on the WHOLE codebase! tests are fast


Order of swaps (each is a separate micro-step):
1. `Ledger::version()` → use `VersionStore` trait
2. Rep weights initialization → accept trait-backed store in `RepWeightsUpdater`
3. Pruning paths → `PrunedStore` + `BlockStore` trait methods
4. Cache initializations (account/confirmed counts) → trait iteration wrappers
5. Remaining sub-stores: `ConfirmationHeightStore`, `AccountStore`, `PendingStore`, `SuccessorStore`, `PeerStore`

Per micro-step checklist:
 - Add/extend trait in `store_api`
 - Implement LMDB adapter + unit tests
 - Implement RocksDB adapter + unit tests (where feasible)
 - Switch exactly one method in `ledger` to use the trait
 - Run: `cargo test -p rsnano_ledger` and then `cargo test`

Exit criteria:
 - All ledger interactions go through `store_api` traits

Rollback:
 - Revert the last method change in `ledger` and keep adapters/tests
