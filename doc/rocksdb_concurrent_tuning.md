# RocksDB Concurrent Writer Tuning (RsNano)

This doc summarizes the safe defaults and knobs for running the concurrent RocksDB writer path, plus quick validation steps.

## Enable the concurrent path
- Use RocksDB backend: set `backend = "rocksdb"` under `[node.storage]`.
- Turn on optimizations: set `[node.experimental] rocksdb_optimizations_enabled = true`. Without this, pipelined writes and concurrent memtable writes are forced off.
- Enable concurrency-friendly RocksDB options (these are defaults when the flag is on, but set explicitly if overriding):
  - `enable_pipelined_write = true`
  - `allow_concurrent_memtable_write = true`
  - `max_open_files = 4096` (or higher if your environment allows it)
  - `write_buffer_size = 134_217_728` (128 MiB; reduce if RAM is constrained)
  - `max_write_buffer_number = 4`
  - `min_write_buffer_number_to_merge = 1`
  - `max_background_jobs = 8`

Example TOML snippet:
```toml
[node.storage]
backend = "rocksdb"

[node.experimental]
rocksdb_optimizations_enabled = true

[node.storage.rocksdb]
max_open_files = 4096
enable_pipelined_write = true
allow_concurrent_memtable_write = true
write_buffer_size = 134_217_728
max_write_buffer_number = 4
min_write_buffer_number_to_merge = 1
max_background_jobs = 8
```

## Threading / batches (8-core baseline)
- Block processor: `block_processor_threads = 6`; batch size 64.
- Confirmation height: threads 2, batch size 128.
- Bounded backlog: threads 2, batch size 128.
Adjust threads down if you see heavy conflicts or IO saturation.

## Telemetry to confirm concurrency
- Ledger-level counters (via StatsCollector):
  - `ledger_write_queue`: `queue_depth`, `waiting_optimistic`, `waiting_pessimistic`, `optimistic_active`, `pessimistic_active`.
  - `ledger_writer`: `optimistic_successes`, `optimistic_conflicts`, `pessimistic_fallbacks`.
  - Per-writer counters: `ledger_writer_{successes|conflicts|fallbacks}.{writer_type}` where writer_type is `block_processor`, `confirmation_height`, `rep_weight_updater`, `voting_finalizer`, `bounded_backlog`, `bootstrap`, `generic`.
- Block processor stats: `block_processor_writer.optimistic_max_concurrency`, `optimistic_successes`, `optimistic_conflicts`, `pessimistic_fallbacks`.
- Use `NANO_LOG_STATS=1` or your existing stats endpoint to inspect these counters.

## Quick validation loop
1. Start node with settings above and `block_processor_threads > 1`.
2. Feed a stream of simple blocks (or use an existing load script).
3. Verify:
   - `ledger_writer_successes.block_processor` grows faster than `ledger_writer_conflicts.block_processor`.
   - `ledger_writer_fallbacks.*` stays near zero.
   - `block_processor_writer.optimistic_max_concurrency` reaches your thread count.
   - `ledger_write_queue.queue_depth` does not steadily climb.
4. If conflicts dominate:
   - Reduce `block_processor_threads` to 4–5.
   - Lower `block_processor_batch_size` to 32.
   - Confirm pipelined/concurrent write options are still enabled (optimizations flag).

## When to toggle off
If you need a rollback to serialized writes, set:
```toml
[node.experimental]
rocksdb_optimizations_enabled = false

[node.storage.rocksdb]
enable_pipelined_write = false
allow_concurrent_memtable_write = false
```
This preserves correctness while disabling the concurrency tuning.
