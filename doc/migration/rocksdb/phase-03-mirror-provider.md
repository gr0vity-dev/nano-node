### Phase 03 (optional): Mirror Provider (test-only)

Objective: Validate RocksDB correctness early without mixing core stores in production.

Rationale:
- Original plan suggested “use RocksDB early.” Mixing core ledger stores (blocks/accounts/etc.) across LMDB and RocksDB in production breaks atomicity and risks subtle invariants.
- Instead, we introduce a test-only provider that writes to both backends and compares reads for the swapped trait methods, giving early signal on RocksDB parity.

Design sketch:
- `MirrorProvider<Lmdb, Rocks>`
  - ReadTxn: contains LMDB read txn (+ optional Rocks snapshot)
  - WriteTxn: contains LMDB write txn + Rocks `WriteBatch`
  - Commit: commit LMDB then write+flush Rocks
  - Expose traits implemented to forward to both; read path can compare and assert (test-only)

Scope:
- Use only in conformance tests per trait (VersionStore, RepWeightStore, PrunedStore, etc.)
- Do not wire into runtime

Commands:
- `cargo test -p store_rocksdb`
- `cargo test -p rsnano_store_lmdb`

Exit criteria:
- Conformance tests for each trait pass on both providers
