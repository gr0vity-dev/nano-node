### RocksDB Migration Roadmap

This folder is the single source of truth for migrating storage from LMDB to RocksDB in tiny, testable steps. It serves as persistent, in-repo memory to avoid context loss and ensure we can mechanically execute each phase.

- **Goals**
  - **Decouple storage** behind traits with minimal churn
  - **Test every small step** (target ≤ 50 LOC code edits per step)
  - **Keep tests green**; avoid modifying tests unless strictly necessary
  - **Introduce RocksDB early** with the smallest usable slice

- **How to use**
  - Read the current phase file listed in `status.json`
  - Execute the listed edits and commands in order
  - Update `status.json` after completing a step
  - If a step fails, use the rollback notes in the phase file

- **Phases**
  - `phase-01-core-traits.md`: Minimal store API and contracts
  - `phase-02-lmdb-adapter.md`: LMDB adapters that implement the API
  - `phase-03-rocksdb-mvp.md`: Minimal RocksDB implementation (VersionStore first, transactional semantics)
  - `phase-03-mirror-provider.md`: Test-only dual-write provider to validate Rocks parity early
  - `phase-04-ledger-integration.md`: Swap ledger callsites incrementally (LMDB provider underneath)
  - `phase-05-switch-and-cleanup.md`: Constructor flip, defaults, cleanup

Key principles
- Do not mix core ledger stores (blocks, accounts, successors, confirmation heights, pending, pruned) across backends in production. Mixing breaks atomicity and invites correctness bugs.
- Use RocksDB early only for meta stores (e.g., `VersionStore`) and only via test harnesses (mirror provider) until the trait surface is complete.
- RocksDB implementations must honor transaction boundaries: stage writes, commit atomically, and provide defined read semantics (snapshots or documented guarantees).

See `commands.md` for the exact cargo invocations to run after each step.
