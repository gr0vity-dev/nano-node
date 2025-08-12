### Iteration 02 — RepWeightStore trait + LMDB adapter + ledger init swap

Constraints:
- ≤ 50 LOC per file
- Do not modify tests
- Always run `cargo test` repo-wide

Deliverables:
1) Add `RepWeightStore` trait to `store_api`
   - Minimal methods needed by `RepWeightsUpdater` init path
2) Implement LMDB adapter for `RepWeightStore`
3) Swap only the rep-weights initialization in `Ledger::new` to use the trait (LMDB provider under the hood)

Commands:
- `cargo test -p store_api`
- `cargo test -p rsnano_store_lmdb`
- `cargo test -p rsnano_ledger`
- `cargo test`

Exit criteria:
- All tests pass
- Ledger still runs on LMDB; trait used for rep weights init only

Rollback:
- Revert the ledger method swap; keep trait and adapter for next iteration
