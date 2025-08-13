### RocksDB Migration Roadmap

This folder is the single source of truth for migrating storage from LMDB to RocksDB in tiny, testable steps. It serves as persistent, in-repo memory to avoid context loss and ensure we can mechanically execute each phase.

- **Goals**
  - **LMDB abstraction first**: 100% of ledger interactions go through `store_api` traits
  - **Micro-steps**: ≤ 50 LOC per edited file; run repo-wide tests after each
  - **Keep tests green**; do not modify tests
  - **Transactional correctness**: RocksDB must stage writes and commit atomically

- **Non-negotiable constraints**
  - Do not mix core stores across backends at runtime (blocks, accounts, successors, confirmation heights, pending, pruned)
  - `store_api` is backend-agnostic (no LMDB/Rocks types; no `as_any` downcasting)
  - Use `StoreProvider` associated types for transactions and stores; avoid `dyn` for generic store traits

- **How to use**
  - Read the current phase file listed in `status.json`
  - Execute the listed edits and commands in order
  - Update `status.json` after completing a step
  - If a step fails, use the rollback notes in the phase file and pick a smaller swap

- **Phases**
  - `phase-01-core-traits.md`: Minimal store API and contracts (associated types; no `as_any`)
  - `phase-02-lmdb-adapter.md`: LMDB adapters that implement the API (wrapper types to satisfy orphan rules)
  - `phase-03-rocksdb-mvp.md`: Minimal RocksDB implementation (VersionStore first, transactional semantics)
  - `phase-03-mirror-provider.md`: Test-only dual-write provider to validate Rocks parity early
  - `phase-04-ledger-integration.md`: Swap ledger callsites incrementally (LMDB provider underneath)
  - `phase-05-switch-and-cleanup.md`: Provider selection, defaults, cleanup

Current trait surface (growing as we migrate)
- Transactions: `TransactionLike`, `ReadTxnLike`, `WriteTxnLike` (commit via provider)
- Stores: `VersionStore`, `PrunedStore`, `BlockStore` (exists/get/del), `AccountStore` (count/get/iter),
  `ConfirmationHeightStore` (count/get/iter), `PendingStore` (get/exists), `SuccessorStore` (get)
  - Next to add: `RepWeightStore` (full usage), `PeerStore`, `FinalVoteStore`, `OnlineWeightStore`

See `commands.md` for the exact cargo invocations to run after each step.
