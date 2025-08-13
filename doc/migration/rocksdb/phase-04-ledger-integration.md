### Phase 04: Ledger Integration (Micro-swaps)

Objective: Transition `ledger` off LMDB types in tiny increments by swapping individual callsites to trait-based access. Keep LMDB as the concrete provider until the minimal surface is complete. Do not mix core stores across backends in production.
ALWAYS RUN cargo test on the WHOLE codebase! tests are fast

Order of swaps (each is a separate micro-step):
1. `Ledger::version()` → use `VersionStore`
2. Pruning (safe subset) → `PrunedStore` + minimal `BlockStore` (exists/get/del) in non-hot code
3. Cache initialization → `AccountStore::iter` and `ConfirmationHeightStore::iter`
4. Helpers/sets (read-only) → `BlockStore` reads in representative finder and sets; `SuccessorStore::get`
5. Pending (read-only) → `PendingStore::get/exists` in non-iterator paths
6. Writes (safe subset) → Genesis path: `BlockStore::put`, `AccountStore::put`, `ConfirmationHeightStore::put`; simple account updates
7. Remaining reads/writes → gradual migration in cementation/rollback once surface is complete and stable
8. Add missing stores and wire as needed → `RepWeightStore`, `PeerStore`, `FinalVoteStore`, `OnlineWeightStore`

Per micro-step checklist:
- Add/extend trait in `store_api` (backend-agnostic)
- Implement LMDB adapter + unit tests
- Implement RocksDB adapter stub (optional) to keep repo building
- Switch exactly one method in `ledger` to use the trait
- Run: `cargo test -p rsnano_ledger` and then `cargo test`

Exit criteria:
- All ledger interactions go through `store_api` traits

Rollback:
- Revert the last method change in `ledger` and keep adapters/tests
