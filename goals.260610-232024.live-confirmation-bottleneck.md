# Feature

Find the first pipeline stage that saturates or creates growing backlog for a fully synced live node under sustained valid live block pressure, and prove whether that stage limits block processing throughput, block confirmation throughput, or both.

The goal is not to optimize throughput yet. The goal is to build an evidence-backed pressure characterization suite and report that makes the next optimization target obvious.

# Verified Current Flow

Roadmap note: `goals.roadmap-overview.md` is not present in this checkout. The verified architecture rules come from `AGENTS.md`, `CLAUDE.md`, and the codepaths below.

```text
live publish message
├─ owner: NetworkMessageProcessor
│  ├─ validates entry work
│  ├─ chooses BlockSource::LiveOriginator or BlockSource::Live
│  ├─ drops live publish while bootstrapper.is_bootstrapping()
│  └─ mutates BlockProcessorQueue by pushing BlockContext
│
├─ owner: BlockProcessorQueue / ProcessQueue / FairQueue
│  ├─ mutable fact: queued blocks grouped by (BlockSource, ChannelId)
│  ├─ source service share:
│  │  ├─ Live and LiveOriginator use priority_live
│  │  ├─ Bootstrap and Unchecked use priority_bootstrap
│  │  ├─ Local uses priority_local
│  │  └─ Forced uses priority_system
│  ├─ capacity:
│  │  ├─ Live and LiveOriginator use max_peer_queue
│  │  └─ Bootstrap, Unchecked, Local, Forced use max_system_queue
│  └─ output: VecDeque<Arc<BlockContext>> from next_batch()
│
├─ owner: BlockBatchProcessor
│  ├─ rolls back Forced competitors before ledger processing
│  ├─ calls Ledger::process_batch(batch.iter().map(|c| (&c.block, c.source)))
│  ├─ records progress/error/source stats
│  ├─ on Ok: asks UncheckedBlockReenqueuer to enqueue blocks waiting on this hash
│  ├─ on non-bootstrap GapPrevious: inserts block into UncheckedMap keyed by previous hash
│  └─ on non-bootstrap GapSource: inserts block into UncheckedMap keyed by source/link hash
│
├─ owner: Ledger::process_batch
│  ├─ validates all batch entries under a read transaction
│  ├─ inserts accepted blocks under a write transaction
│  ├─ computes ProcessResult.priority during insertion
│  ├─ commits the write transaction
│  └─ emits LedgerEvent::BlocksProcessed(Vec<ProcessResult>)
│
├─ owner: LedgerEventProcessor
│  ├─ on BlocksProcessed:
│  │  ├─ increments block processed event counters
│  │  ├─ requeues deferred ConfirmingSet blocks
│  │  ├─ updates fork cache
│  │  └─ publishes NodeEvent::BlocksProcessed when configured
│  └─ on BlocksConfirmed:
│     ├─ increments block confirmed event counters
│     └─ asks DependentElectionsConfirmer to confirm dependent elections
│
├─ owner: ElectionSchedulers
│  ├─ on BlocksProcessed Ok:
│  │  ├─ extracts account from result.saved_block
│  │  ├─ discards ProcessResult.priority for activation
│  │  └─ enqueues account into prio_sched_queue
│  ├─ on BlocksConfirmed:
│  │  ├─ enqueues confirmed block account
│  │  └─ enqueues nonzero destination account when different
│  └─ output: EventProcessor<Account> calls PriorityScheduler::activate
│
├─ owner: PriorityScheduler
│  ├─ reads ledger any-set for account and confirmation height
│  ├─ finds next unconfirmed block for the account
│  ├─ rejects candidates whose dependencies are not confirmed
│  ├─ recomputes BlockPriority from ledger state with any.block_priority(&block)
│  ├─ inserts candidate into PriorityBuckets
│  └─ notifies its condition when insertion succeeds
│
├─ owner: PriorityBuckets / Bucket
│  ├─ mutable fact: queued consensus candidates grouped by balance bucket
│  ├─ insert chooses bucket by priority.balance
│  ├─ each Bucket stores candidates ordered by priority
│  ├─ full bucket evicts the lowest priority candidate only if the new candidate is higher
│  └─ next_candidate returns highest priority candidate only when vacancy exists or it beats lowest active priority
│
├─ owner: AecService / ActiveElectionsContainer
│  ├─ PriorityScheduler predicate asks AEC whether the candidate source should schedule
│  ├─ refill iterates AEC buckets from high bucket id to low bucket id
│  ├─ if AEC is globally full, bucket_vacancy is zero
│  ├─ if target AEC bucket is full, erase_lowest_prio_election(candidate.bucket_id)
│  ├─ inserts candidate as a priority election
│  └─ records activation success, duplicate, confirmed, and replacement stats
│
└─ owner: ConfirmingSet / Ledger confirmation path
   ├─ confirmed elections are added to ConfirmingSet
   ├─ ConfirmingSet batches confirmation roots
   ├─ Ledger::confirm_batch durably updates confirmation height
   └─ Ledger emits LedgerEvent::BlocksConfirmed
```

# Relevant Files

The implementation and validation path materially involves:

- `AGENTS.md`
- `CLAUDE.md`
- `node/src/transport/network_message_processor.rs`
- `node/src/block_processing/process_queue.rs`
- `node/src/block_processing/block_batch_processor.rs`
- `node/src/block_processing/unchecked_map/mod.rs`
- `node/src/block_processing/unchecked_map/reenqueuer.rs`
- `ledger/src/ledger.rs`
- `ledger/src/lib.rs`
- `node/src/ledger_event_processor.rs`
- `node/src/consensus/election_schedulers/mod.rs`
- `node/src/consensus/election_schedulers/priority/priority_scheduler.rs`
- `node/src/consensus/election_schedulers/priority/priority_buckets.rs`
- `node/src/consensus/election_schedulers/priority/bucket.rs`
- `node/src/consensus/active_elections/aec_service.rs`
- `node/src/consensus/active_elections/active_elections_container.rs`
- `node/src/cementation/confirming_set.rs`
- `tools/test_helpers`

# How To Run The Pressure Suite

Run the regular deterministic pressure characterization tests:

```sh
cargo test -p rsnano_node block_processing::process_queue
cargo test -p rsnano_node consensus::active_elections::active_elections_container
cargo test -p rsnano_node cementation::confirming_set
cargo test -p rsnano_node consensus::election_schedulers::priority::priority_scheduler -- --nocapture
cargo test -p rsnano_ledger real_lmdb_ -- --nocapture
```

Run the full local confirmation timing slice only:

```sh
cargo test -p rsnano_node pressure_full_local_confirmation_path -- --nocapture
```

Run the explicit 10k AecFactProcessor/backpressure benchmark. This test is ignored by default because it is a local pressure benchmark, not a normal unit-test cost:

```sh
cargo test -p rsnano_node pressure_aec_fact_processor_thread_dispatch_reports_10k_across_10_peers_timing -- --ignored --nocapture
```

Expected report shape from the 10-peer / 10k benchmark:

```text
aec_fact_thread_dispatch_10k_10_peers blocks=10000 peers=10 confirmed=10000 dequeue_us=<local> process_us=<local> aec_fact_thread_dispatch_us=<local> cement_us=<local> slowest_stage=<stage>
```

Interpretation rule:

```text
timing result
├─ compare stages from the same run only
├─ treat wall-clock values as local machine measurements
├─ do not weaken admission, work, signature, dependency, or ledger validation invariants to improve the number
└─ use this suite to choose an optimization owner, not as proof that network-wide confirmation is solved
```

# Required Product Rules

```text
goal workload
└─ fully synced live node
   ├─ not in bootstrap mode
   ├─ valid live blocks
   ├─ input rate intentionally exceeds eventual confirmation capacity
   └─ result must distinguish accepted blocks from confirmed blocks
```

The goal is met only when the investigation can make these claims with repository evidence and measurements:

1. The first saturating stage is identified for the named workload.
2. The identified stage has an owner in the codebase.
3. The proof includes backlog growth, throughput, and latency for that stage.
4. The proof says whether the stage limits block acceptance, confirmation, or both.
5. The proof separates source-based FairQueue priority from consensus BlockPriority.
6. The proof measures priority fidelity after ledger insertion, where PriorityScheduler recomputes BlockPriority.
7. The proof uses deterministic tests or benchmarks where the boundary does not require real network timing.
8. Disk/LMDB pressure is measured at the real ledger boundary when claiming ledger write pressure.
9. Confirmation throughput is measured at or beyond ConfirmingSet / Ledger confirmation, not inferred from block processing.
10. The final report recommends the next engineering change for the top bottleneck and names the component to change.

Every pressure test or benchmark must be scored against these KPIs:

```text
KPI
├─ backlog visibility
│  └─ shows which queue or set grows
├─ throughput visibility
│  └─ reports blocks/sec or confirmations/sec at the relevant boundary
├─ latency visibility
│  └─ reports wait time before processing, activation, or confirmation
├─ source fairness
│  └─ shows whether Live, Bootstrap, Unchecked, Local, or Forced traffic starves another source
├─ priority fidelity
│  └─ shows whether high consensus priority activates sooner after ledger processing
├─ boundary realism
│  └─ crosses CPU, LMDB, clock, or runtime boundary only when that boundary is the claim
├─ determinism
│  └─ avoids sleeps, races, real peer network timing, and interaction mocks
└─ actionability
   └─ failure or slowdown names the owner to change
```

# Forbidden Outcomes

```text
forbidden conclusions
├─ "tests pass, so bottleneck is understood"
├─ "block processing is fast, so confirmation is fast"
├─ "FairQueue source priority proves consensus priority fidelity"
├─ "ProcessResult.priority proves activation order"
├─ "one broad node-level load result proves the owning stage"
├─ "bootstrap catch-up behavior is mixed into the live-synced conclusion"
├─ "network timing races are used as proof for deterministic scheduling behavior"
├─ "mock expectations are used to prove throughput or event choreography"
└─ "new instrumentation creates a second owner for queue, priority, or confirmation state"
```

Do not stop with only the current `ProcessQueue` invariants. Those tests prove source-share behavior and queue capacity behavior, but they do not expose the saturation point.

# ASCII Implementation Plan

```text
1. name the workload and baseline counters
├─ introduce a test/bench naming convention for live synced pressure characterization
├─ define one result record shape for measurements:
│  ├─ stage
│  ├─ input count
│  ├─ output count
│  ├─ backlog before/after
│  ├─ latency or deterministic dequeue/activation distance
│  └─ source/priority classification when relevant
└─ tests prove the metrics are computed from observable state, not call expectations

2. characterize ProcessQueue pressure
├─ owner: node/src/block_processing/process_queue.rs
├─ use production ProcessQueue and BlockContext
├─ avoid threads, sleeps, network sockets, and mocks
├─ scenarios:
│  ├─ live queue under sustained bootstrap backlog
│  ├─ live queue under sustained unchecked backlog
│  ├─ mixed Live and LiveOriginator peer queues
│  └─ Forced/System traffic competing with live traffic
├─ measure:
│  ├─ dequeue distance for live blocks
│  ├─ source output counts per batch/window
│  └─ queue growth by source
└─ stopping evidence:
   └─ ProcessQueue either is or is not the first backlog source under synthetic mixed-source pressure

3. characterize Ledger::process_batch pressure
├─ owner: ledger/src/ledger.rs
├─ use Ledger::new_null for deterministic logic-path checks
├─ use a real temporary LMDB-backed ledger benchmark before claiming disk/write pressure
├─ scenarios:
│  ├─ valid sequential live blocks
│  ├─ valid blocks across many accounts
│  └─ gap/fork/error mix only as separate non-live-synced modes
├─ measure:
│  ├─ validation time
│  ├─ insert/write/commit time when separable
│  ├─ processed blocks/sec
│  └─ emitted BlocksProcessed count
└─ stopping evidence:
   └─ ledger is proven or ruled out as the first block-acceptance limiter for the workload

4. characterize scheduler activation lag
├─ owners:
│  ├─ ElectionSchedulers
│  ├─ PriorityScheduler
│  └─ PriorityBuckets
├─ use production scheduler/bucket code with nullable ledger/clock/AEC where possible
├─ scenarios:
│  ├─ many successful ProcessResult accounts enqueued for activation
│  ├─ high-priority and low-priority accounts interleaved after ledger insertion
│  └─ dependencies-confirmed false prevents activation
├─ measure:
│  ├─ prio_sched_queue backlog
│  ├─ account activation count
│  ├─ bucket insertion count/drop/duplicate/full counters
│  └─ distance from BlocksProcessed to candidate available for AEC
└─ stopping evidence:
   └─ scheduler lag is proven or ruled out as the first confirmation limiter after ledger acceptance

5. characterize AEC full and replacement behavior
├─ owners:
│  ├─ AecService
│  └─ ActiveElectionsContainer
├─ use production AEC refill with a deterministic ElectionCandidateSource
├─ scenarios:
│  ├─ AEC has vacancy
│  ├─ AEC globally full but candidate does not beat lowest active priority
│  ├─ AEC globally full and candidate beats lowest active priority
│  └─ high bucket and low bucket candidates available in same refill pass
├─ measure:
│  ├─ activation success
│  ├─ replacement count
│  ├─ duplicate rejection
│  ├─ per-bucket active counts
│  └─ candidate left queued vs inserted
└─ stopping evidence:
   └─ AEC slot pressure and replacement churn are proven or ruled out as the first confirmation limiter

6. characterize ConfirmingSet and ledger confirmation pressure
├─ owners:
│  ├─ ConfirmingSet
│  └─ Ledger confirmation path
├─ use deterministic confirmation roots already present in ledger
├─ use real ledger boundary when claiming confirmation write pressure
├─ scenarios:
│  ├─ confirmed elections entering ConfirmingSet faster than batch confirmation
│  ├─ deferred confirmation requeued after BlocksProcessed
│  └─ dependent confirmations after BlocksConfirmed
├─ measure:
│  ├─ ConfirmingSet set/deferred/current size
│  ├─ confirmed blocks/sec
│  ├─ confirmation batch latency
│  └─ BlocksConfirmed count
└─ stopping evidence:
   └─ confirmation/cementing is proven or ruled out as the first limiter after AEC election success

7. produce the bottleneck report
├─ include the named workload and environment
├─ include the stage-by-stage table:
│  ├─ input rate
│  ├─ output rate
│  ├─ backlog trend
│  ├─ latency trend
│  └─ owner
├─ identify the first saturating stage
├─ classify the limiter:
│  ├─ block acceptance only
│  ├─ confirmation only
│  └─ both
├─ score each test or benchmark against the KPI set
└─ recommend the next code change for the top bottleneck
```

# Final Target Flow

```text
pressure characterization suite
├─ workload: fully synced valid live block pressure
│
├─ stage probes
│  ├─ publish admission
│  │  └─ accepted/sec and dropped live blocks
│  ├─ ProcessQueue
│  │  └─ source backlog and deterministic dequeue latency
│  ├─ Ledger::process_batch
│  │  └─ accepted/sec, batch time, write/commit pressure
│  ├─ Unchecked feedback
│  │  └─ unchecked size and satisfied/requeued rate in separate gap scenarios
│  ├─ ElectionSchedulers / PriorityScheduler
│  │  └─ activation queue lag, bucket drops, priority fidelity
│  ├─ AEC
│  │  └─ active count, replacement count, election age, per-bucket slot pressure
│  └─ ConfirmingSet / Ledger confirmation
│     └─ confirmed/sec, confirmation backlog, cementing latency
│
├─ evidence report
│  ├─ first growing backlog
│  ├─ first output rate below input rate
│  ├─ first rising latency
│  ├─ owner of the limiting mutable state
│  └─ whether the limiter affects processing, confirmation, or both
│
└─ next engineering goal
   ├─ targets exactly one owner
   ├─ keeps state ownership singular
   ├─ uses TWM-style deterministic tests
   └─ avoids adding fallback paths or duplicate scheduling ownership
```

The relevant files for implementation are listed again here: `node/src/transport/network_message_processor.rs`, `node/src/block_processing/process_queue.rs`, `node/src/block_processing/block_batch_processor.rs`, `ledger/src/ledger.rs`, `ledger/src/lib.rs`, `node/src/ledger_event_processor.rs`, `node/src/consensus/election_schedulers/mod.rs`, `node/src/consensus/election_schedulers/priority/priority_scheduler.rs`, `node/src/consensus/election_schedulers/priority/priority_buckets.rs`, `node/src/consensus/election_schedulers/priority/bucket.rs`, `node/src/consensus/active_elections/aec_service.rs`, `node/src/consensus/active_elections/active_elections_container.rs`, `node/src/cementation/confirming_set.rs`, and `tools/test_helpers`.

# Execution Ledger

## 2026-06-10 ProcessQueue Characterization

Implemented deterministic pressure characterization tests in `node/src/block_processing/process_queue.rs`.

```text
ProcessQueue pressure slice
├─ production path used
│  ├─ ProcessQueue
│  ├─ FairQueue
│  └─ BlockContext
├─ infrastructure crossed
│  └─ none
├─ external boundaries avoided
│  ├─ no network sockets
│  ├─ no ledger or LMDB
│  ├─ no threads
│  ├─ no clocks
│  └─ no mocks or interaction expectations
├─ evidence added
│  ├─ bootstrap + unchecked backlog with default shares delays the second Live dequeue until position 17
│  ├─ Forced backlog with default shares delays the second Live dequeue until position 33
│  └─ Live and LiveOriginator have separate peer fair-queue turns and separate max_peer_queue capacity
└─ command
   └─ cargo test -p rsnano_node block_processing::process_queue
      └─ result: 6 passed, 0 failed
```

KPI score for this slice:

```text
backlog visibility       yes: asserts remaining source_len after dequeue windows
throughput visibility    partial: deterministic service share counts, not blocks/sec
latency visibility       yes: deterministic dequeue position for Live under competing backlog
source fairness          yes: Live, LiveOriginator, Bootstrap, Unchecked, Forced
priority fidelity        no: consensus priority does not exist before ledger insertion
boundary realism         yes for queue logic; no disk/network boundary claimed
determinism              yes: no sleeps, races, network timing, or mocks
actionability            yes for ProcessQueue source-share tuning only
```

Current conclusion:

```text
ProcessQueue is now characterized as a possible pre-ledger live delay source.
It is not yet proven to be the first pipeline bottleneck.
It cannot answer confirmation throughput, because no ledger insertion, scheduler activation,
AEC election insertion, vote quorum, or cementing occurs in this slice.
```

Remaining required slices before this goal can be called complete:

```text
remaining proof
├─ Ledger::process_batch pressure
│  └─ accepted/sec and write/commit pressure at real ledger boundary before claiming disk bottleneck
├─ Scheduler / PriorityScheduler pressure
│  └─ BlocksProcessed-to-candidate lag and post-ledger priority fidelity
├─ AEC full/replacement pressure
│  └─ activation success, replacement count, and queued-vs-inserted candidate behavior
├─ ConfirmingSet / ledger confirmation pressure
│  └─ confirmed/sec and confirmation backlog at or beyond cementing
└─ final bottleneck report
   └─ stage table with first growing backlog, limiter classification, and next owner to change
```

## 2026-06-10 AEC Refill / Replacement Characterization

Implemented deterministic AEC slot-pressure characterization tests in `node/src/consensus/active_elections/active_elections_container.rs`.

```text
AEC pressure slice
├─ production path used
│  ├─ ActiveElectionsContainer::refill
│  ├─ AecInsertRequest::new_priority
│  ├─ ElectionCandidateSource contract
│  └─ real Election insertion/removal inside RootContainer
├─ infrastructure crossed
│  └─ none
├─ external boundaries avoided
│  ├─ no ledger
│  ├─ no LMDB
│  ├─ no network
│  ├─ no threads
│  ├─ no clocks beyond deterministic Timestamp test value
│  └─ no mocks or interaction expectations
├─ evidence added
│  ├─ when an AEC bucket is full, a stronger candidate replaces the lowest-priority active election
│  ├─ when an AEC bucket is full, a weaker candidate remains queued and does not replace the active election
│  ├─ when AEC is globally full, refill still continues across buckets and can replace in a later bucket
│  └─ TimePriority ordering is lower timestamp = higher priority, verified from `types/src/priority.rs`
└─ command
   └─ cargo test -p rsnano_node consensus::active_elections::active_elections_container
      └─ result: 7 passed, 0 failed
```

KPI score for this slice:

```text
backlog visibility       partial: candidate source retains weaker queued candidate
throughput visibility    no: deterministic refill behavior, not elections/sec
latency visibility       partial: one refill pass behavior, not elapsed activation lag
source fairness          no: source fairness is ProcessQueue-owned
priority fidelity        partial: AEC replacement respects TimePriority once candidate source returns candidate
boundary realism         yes for AEC logic; no ledger/network/disk boundary claimed
determinism              yes: no sleeps, races, network timing, or mocks
actionability            yes for AEC slot/replacement behavior
```

Current conclusion:

```text
AEC replacement behavior is now characterized as a confirmation-side pressure point.
AEC does not independently decide candidate eligibility while full; it receives vacancy and lowest active priority,
and the ElectionCandidateSource decides whether to return a candidate. Once a candidate is returned, AEC owns
replacement and insertion.

This still does not identify the whole pipeline bottleneck, because scheduler activation lag, ledger write pressure,
and confirmation/cementing pressure remain unmeasured.
```

## 2026-06-10 PriorityScheduler Activation Characterization

Implemented deterministic scheduler activation characterization tests in `node/src/consensus/election_schedulers/priority/priority_scheduler.rs`.

```text
PriorityScheduler pressure slice
├─ production path used
│  ├─ Ledger::new_null
│  ├─ Ledger::process_one
│  ├─ Ledger::confirm
│  ├─ PriorityScheduler::activate
│  ├─ PriorityBuckets
│  ├─ PriorityScheduler::run_one
│  └─ AecService / ActiveElectionsContainer insertion
├─ infrastructure crossed
│  └─ nullable ledger storage only
├─ external boundaries avoided
│  ├─ no real LMDB
│  ├─ no network
│  ├─ no scheduler thread
│  ├─ no sleeps
│  └─ no mocks or interaction expectations
├─ evidence added
│  ├─ activated accounts are converted into candidates from recomputed ledger BlockPriority
│  ├─ a one-slot AEC receives the stronger same-bucket candidate after scheduler refill
│  ├─ an unconfirmed dependency prevents scheduler bucket insertion
│  ├─ a full PriorityBucket keeps the stronger candidate and evicts the weaker one
│  └─ within one balance bucket, lower/older TimePriority is stronger than newer TimePriority
└─ command
   └─ cargo test -p rsnano_node consensus::election_schedulers::priority::priority_scheduler
      └─ result: 3 passed, 0 failed
```

KPI score for this slice:

```text
backlog visibility       partial: scheduler bucket length and candidate retention are asserted
throughput visibility    no: deterministic activation behavior, not activations/sec
latency visibility       partial: one activation/refill pass, not elapsed queue lag
source fairness          no: source fairness is ProcessQueue-owned
priority fidelity        yes: candidates are selected from recomputed ledger BlockPriority
boundary realism         yes for nullable ledger state; no real disk boundary claimed
determinism              yes: no sleeps, races, network timing, scheduler thread, or mocks
actionability            yes for scheduler bucket ordering, dependency checks, and activation policy
```

Current conclusion:

```text
PriorityScheduler is now characterized as a post-ledger activation pressure point.
The scheduler does not use ProcessResult.priority in these tests; it reads ledger account state,
recomputes BlockPriority through ledger.any(), inserts into PriorityBuckets, and then refills AEC.

This still does not identify the whole pipeline bottleneck, because ledger write pressure and
confirmation/cementing pressure remain unmeasured, and this slice does not report activations/sec.
```

## 2026-06-10 Nullable Ledger Batch Characterization

Implemented a narrow `Ledger::process_batch` characterization test in `ledger/src/ledger.rs`.

```text
ledger batch slice
├─ production path used
│  ├─ Ledger::new_null
│  ├─ Ledger::process_one for setup sends
│  ├─ Ledger::confirm for source dependencies
│  ├─ Ledger::process_batch for valid live open blocks
│  └─ LedgerEvent::BlocksProcessed publisher
├─ infrastructure crossed
│  └─ nullable ledger storage only
├─ external boundaries avoided
│  ├─ no real LMDB
│  ├─ no network
│  ├─ no threads
│  └─ no mocks or interaction expectations
├─ evidence added
│  ├─ valid live batch returns one ProcessResult per block
│  ├─ successful results include saved blocks
│  ├─ BlockSource is preserved through process_batch results
│  ├─ computed priorities are present for successful inserts
│  └─ BlocksProcessed event is emitted for the processed batch
└─ command
   └─ cargo test -p rsnano_ledger process_batch_reports_valid_live_blocks_and_emits_processed_event
      └─ result: 1 passed, 0 failed
```

KPI score for this slice:

```text
backlog visibility       no: Ledger::process_batch has no queue backlog
throughput visibility    no: no blocks/sec measurement
latency visibility       no: no batch timing measurement
source fairness          partial: source is preserved, but fairness is ProcessQueue-owned
priority fidelity        partial: successful inserts expose computed priority, but scheduler recomputation is tested separately
boundary realism         partial: production ledger path with nullable storage; no real LMDB boundary
determinism              yes: no sleeps, races, network timing, or mocks
actionability            partial: proves accounting/event output, not write bottleneck
```

Current conclusion:

```text
Ledger::process_batch accounting and event emission are now characterized for valid live blocks.
This does not satisfy the goal's disk/write-pressure requirement, because it used nullable storage.
A real temporary LMDB-backed benchmark or measurement remains required before claiming ledger write pressure.
```

## 2026-06-10 ConfirmingSet Cementing Characterization

Implemented deterministic ConfirmingSet cementing characterization tests in `node/src/cementation/confirming_set.rs`.

```text
ConfirmingSet pressure slice
├─ production path used
│  ├─ ConfirmingSetThread::run_batch
│  ├─ Ledger::confirm_batch
│  ├─ CementedNotifier
│  ├─ ConfirmingSet current set clearing
│  └─ ConfirmingSet deferred set insertion on failure
├─ infrastructure crossed
│  └─ nullable ledger storage only
├─ external boundaries avoided
│  ├─ no real LMDB
│  ├─ no network
│  ├─ no background confirming thread
│  ├─ no sleeps
│  └─ no mocks or interaction expectations
├─ evidence added
│  ├─ a queued confirmation root is cemented through Ledger::confirm_batch
│  ├─ current in-flight confirmation state is cleared after the batch
│  ├─ a missing confirmation root is moved to deferred
│  └─ ledger confirmed-state changes are asserted through the production LedgerSet API
└─ command
   └─ cargo test -p rsnano_node cementation::confirming_set
      └─ result: 3 passed, 0 failed
```

KPI score for this slice:

```text
backlog visibility       partial: current and deferred sets are asserted
throughput visibility    no: no confirmations/sec measurement
latency visibility       no: no batch timing measurement
source fairness          no: source fairness is ProcessQueue-owned
priority fidelity        no: priority has already been converted into confirmed roots before this stage
boundary realism         partial: production cementing path with nullable storage; no real LMDB boundary
determinism              yes: no sleeps, races, network timing, confirming thread, or mocks
actionability            partial: proves cementing state transitions, not confirmation throughput
```

Current conclusion:

```text
ConfirmingSet cementing behavior is now characterized for deterministic success and failure paths.
This does not satisfy the goal's throughput requirement, because it does not measure confirmations/sec,
batch latency, or real ledger write pressure.
```

## 2026-06-10 Real LMDB Ledger Batch Timing

Implemented a real LMDB-backed `Ledger::process_batch` timing characterization test in `ledger/src/ledger.rs`.

```text
real LMDB ledger slice
├─ production path used
│  ├─ LedgerBuilder::new
│  ├─ real LmdbEnvironmentFactory default path through LedgerBuilder
│  ├─ Ledger::process_one for setup sends
│  ├─ Ledger::confirm for source dependencies
│  └─ Ledger::process_batch for valid live open blocks
├─ infrastructure crossed
│  └─ real temporary LMDB file under system temp directory
├─ external boundaries avoided
│  ├─ no network
│  ├─ no scheduler thread
│  ├─ no sleeps
│  └─ no mocks or interaction expectations
├─ evidence added
│  ├─ valid live batch returns one ProcessResult per block against real LMDB storage
│  ├─ successful results include saved blocks
│  ├─ source remains BlockSource::Live
│  ├─ elapsed process_batch time is measured with std::time::Instant
│  └─ the test prints a local timing report under --nocapture
└─ command
   └─ cargo test -p rsnano_ledger real_lmdb_process_batch_reports_batch_timing_for_valid_live_blocks -- --nocapture
      └─ result: 1 passed, 0 failed
```

Observed local run:

```text
ledger_process_batch_lmdb blocks=64 elapsed_us=19253 blocks_per_sec=3324.00
```

KPI score for this slice:

```text
backlog visibility       no: Ledger::process_batch has no queue backlog
throughput visibility    yes: reports blocks/sec for a real LMDB-backed batch
latency visibility       yes: reports elapsed process_batch time
source fairness          no: all measured blocks are live; source fairness is ProcessQueue-owned
priority fidelity        partial: successful inserts compute priority, but scheduler recomputation is tested separately
boundary realism         yes: crosses real LMDB storage boundary
determinism              partial: deterministic workload, but timing depends on local machine/storage state
actionability            partial: gives a ledger batch baseline, not yet compared against other stage rates
```

Current conclusion:

```text
Ledger block-processing throughput now has a real LMDB-backed local baseline for this workload.
This still does not identify the first whole-pipeline bottleneck, because confirmation/cementing
throughput has not yet been timed against a real ledger boundary and no final stage comparison report exists.
```

## 2026-06-10 Real LMDB Confirmation Timing

Implemented a real LMDB-backed `Ledger::confirm_batch` timing characterization test in `ledger/src/ledger.rs`.

```text
real LMDB confirmation slice
├─ production path used
│  ├─ LedgerBuilder::new
│  ├─ real LmdbEnvironmentFactory default path through LedgerBuilder
│  ├─ Ledger::process_one for setup sends and opens
│  ├─ Ledger::confirm for source dependencies
│  └─ Ledger::confirm_batch for valid live receive/open blocks
├─ infrastructure crossed
│  └─ real temporary LMDB file under system temp directory
├─ external boundaries avoided
│  ├─ no network
│  ├─ no scheduler thread
│  ├─ no sleeps
│  └─ no mocks or interaction expectations
├─ evidence added
│  ├─ confirm_batch durably confirms all measured block hashes
│  ├─ no measured block is reported already-confirmed
│  ├─ no measured block is reported cementing-failed
│  ├─ elapsed confirm_batch time is measured with std::time::Instant
│  └─ the test prints a local timing report under --nocapture
└─ command
   └─ cargo test -p rsnano_ledger real_lmdb_ -- --nocapture
      └─ result: 2 passed, 0 failed
```

Observed local paired run:

```text
ledger_process_batch_lmdb blocks=64 elapsed_us=19206 blocks_per_sec=3332.23
ledger_confirm_batch_lmdb blocks=64 elapsed_us=3370 blocks_per_sec=18988.52
```

KPI score for this slice:

```text
backlog visibility       no: direct ledger confirmation has no ConfirmingSet queue backlog
throughput visibility    yes: reports confirmations/sec for a real LMDB-backed batch
latency visibility       yes: reports elapsed confirm_batch time
source fairness          no: source fairness is ProcessQueue-owned
priority fidelity        no: priority has already been converted into confirmation roots before this stage
boundary realism         yes: crosses real LMDB storage boundary
determinism              partial: deterministic workload, but timing depends on local machine/storage state
actionability            yes for ledger processing-vs-confirmation comparison within this narrow workload
```

Current local ledger-only comparison:

```text
For the measured 64-block real-LMDB workload, Ledger::process_batch is slower than Ledger::confirm_batch:
├─ process_batch: 3332.23 blocks/sec
└─ confirm_batch: 18988.52 blocks/sec

This suggests ledger block insertion is a stronger local ledger-boundary pressure point than ledger cementing
for this narrow workload. It does not yet prove the whole-node bottleneck because ProcessQueue, scheduler,
AEC, and ConfirmingSet have not been measured in one comparable end-to-end pressure run.
```

## 2026-06-10 Comparable Live-Synced Pipeline Timing

Implemented a narrow comparable live-synced pipeline timing test in `node/src/consensus/election_schedulers/priority/priority_scheduler.rs`.

```text
comparable live-synced pipeline slice
├─ production path used
│  ├─ BlockProcessorQueue::push
│  ├─ BlockProcessorQueue::pop_blocking
│  ├─ real LMDB-backed Ledger::process_batch
│  ├─ PriorityScheduler::activate
│  ├─ PriorityBuckets
│  ├─ PriorityScheduler::run_one
│  ├─ AecService / ActiveElectionsContainer insertion
│  └─ real LMDB-backed Ledger::confirm_batch
├─ infrastructure crossed
│  └─ real temporary LMDB file under system temp directory
├─ external boundaries avoided
│  ├─ no real peer network timing
│  ├─ no scheduler thread
│  ├─ no confirming thread
│  ├─ no sleeps
│  └─ no mocks or interaction expectations
├─ evidence added
│  ├─ the same 64 valid live blocks are measured across dequeue, processing, scheduling, and confirmation
│  ├─ accepted block count is asserted at Ledger::process_batch
│  ├─ scheduled election count is asserted at AEC
│  ├─ confirmed block count is asserted through the ledger confirmed set
│  └─ the slowest comparable stage is reported as first_limiter
└─ command
   └─ cargo test -p rsnano_node pressure_live_synced_pipeline_reports_comparable_stage_timings -- --nocapture
      └─ result: 1 passed, 0 failed
```

Observed local run:

```text
live_synced_pipeline blocks=64 dequeue_us=53 process_us=19805 schedule_us=2854 confirm_us=3519 first_limiter=process_batch
```

KPI score for this slice:

```text
backlog visibility       partial: queue is drained and AEC count is asserted, but no sustained growing backlog window is generated
throughput visibility    yes: comparable elapsed times are reported for a fixed 64-block input batch
latency visibility       yes: per-stage elapsed batch time is reported
source fairness          no: all workload blocks are BlockSource::Live
priority fidelity        yes: scheduling uses PriorityScheduler::activate and recomputes ledger BlockPriority
boundary realism         yes: crosses real LMDB for processing and confirmation; avoids real network timing
determinism              partial: deterministic workload and no sleeps, but timing depends on local machine/storage state
actionability            yes: names Ledger::process_batch as the slowest owner in this narrow measured pipeline
```

Current local comparable-pipeline comparison:

```text
For the measured 64-block live-synced pipeline slice, Ledger::process_batch is the slowest stage:
├─ queue dequeue: 53 us
├─ process_batch: 19805 us
├─ scheduler + AEC activation: 2854 us
└─ confirm_batch: 3519 us

This identifies Ledger::process_batch as the first measured throughput limiter for this narrow serial
pipeline. It limits block acceptance and therefore indirectly caps confirmation input for this workload.

It still does not prove sustained backlog growth, because the test processes one fixed batch rather than
running repeated windows with an input rate above measured output capacity.
```

## 2026-06-10 Sustained Live-Synced Backlog Growth

Implemented a repeated-window live-synced pressure test in `node/src/consensus/election_schedulers/priority/priority_scheduler.rs`.

```text
sustained live-synced pressure slice
├─ workload
│  ├─ 3 deterministic service windows
│  ├─ 128 valid live blocks arrive per window
│  ├─ one 64-block ProcessQueue batch is serviced per window
│  ├─ every serviced block crosses real LMDB Ledger::process_batch
│  ├─ every accepted block is offered to PriorityScheduler/AEC
│  └─ every accepted block crosses real LMDB Ledger::confirm_batch
├─ production path used
│  ├─ BlockProcessorQueue::push
│  ├─ BlockProcessorQueue::pop_blocking
│  ├─ real LMDB-backed Ledger::process_batch
│  ├─ PriorityScheduler::activate
│  ├─ PriorityScheduler::run_one
│  ├─ AecService / ActiveElectionsContainer insertion
│  └─ real LMDB-backed Ledger::confirm_batch
├─ external boundaries avoided
│  ├─ no real peer network timing
│  ├─ no background scheduler thread
│  ├─ no background confirming thread
│  ├─ no sleeps
│  └─ no mocks or interaction expectations
└─ command
   └─ cargo test -p rsnano_node pressure_live_synced_pipeline -- --nocapture
      └─ result: 2 passed, 0 failed
```

Observed local run:

```text
live_synced_backlog windows=3 arrivals_per_window=128 serviced_per_window=64 backlog_after_windows=[64, 128, 192] process_us=58283 schedule_us=7546 confirm_us=10751 first_limiter=process_batch
```

KPI score for this slice:

```text
backlog visibility       yes: ProcessQueue backlog grows 64 -> 128 -> 192 under the named workload
throughput visibility    yes: 384 arrivals, 192 processed/confirmed, and per-stage elapsed times are reported
latency visibility       yes: backlog growth implies rising wait before ledger processing; per-stage batch time is reported
source fairness          no: intentionally single-source BlockSource::Live workload
priority fidelity        yes: accepted blocks pass through PriorityScheduler::activate and recomputed ledger BlockPriority
boundary realism         yes: real LMDB is used for process_batch and confirm_batch
determinism              partial: deterministic workload and no sleeps, but timing depends on local machine/storage state
actionability            yes: first limiter is owned by Ledger::process_batch for this workload
```

Current sustained-pressure conclusion:

```text
Under the named fully synced valid-live workload, arrivals intentionally exceed the measured service rate.
The first growing backlog is visible in BlockProcessorQueue immediately before ledger processing:
├─ after window 1: 64 queued live blocks
├─ after window 2: 128 queued live blocks
└─ after window 3: 192 queued live blocks

The slowest measured serviced stage in the same run is Ledger::process_batch:
├─ process_batch: 58283 us total
├─ scheduler + AEC activation: 7546 us total
└─ confirm_batch: 10751 us total

The owning bottleneck is Ledger::process_batch at the real LMDB boundary.
It directly limits block acceptance throughput and indirectly limits confirmation throughput by limiting
how many valid live blocks become scheduler/AEC/confirmation input.
```

## 2026-06-11 Full Local Confirmation Path Timing

Implemented local full-confirmation timing tests in `node/src/consensus/election_schedulers/priority/priority_scheduler.rs`.

```text
full local confirmation slice
├─ workload
│  ├─ one valid live block
│  └─ one 256-block valid live batch
├─ production path used
│  ├─ BlockProcessorQueue::push
│  ├─ BlockProcessorQueue::pop_blocking
│  ├─ real LMDB-backed Ledger::process_batch
│  ├─ PriorityScheduler::activate
│  ├─ PriorityScheduler::run_one
│  ├─ AecService / ActiveElectionsContainer
│  ├─ AecVoter::tick
│  ├─ VoteGenerators worker threads
│  ├─ VoteBroadcaster
│  ├─ VoteProcessorQueue
│  ├─ VoteProcessor worker thread
│  ├─ VoteApplier / AEC apply_vote / quorum transition
│  ├─ AecFact::ElectionConfirmed
│  ├─ AecFactProcessor::process
│  ├─ ConfirmingSet::add through the production fact processor
│  ├─ ConfirmingSet confirmation-height thread
│  └─ real LMDB-backed Ledger::confirm_batch
├─ external boundaries avoided
│  └─ no real peer network transport
└─ command
   └─ cargo test -p rsnano_node pressure_full_local_confirmation_path -- --nocapture
      └─ result: 2 passed, 0 failed
```

Observed local one-block run:

```text
full_local_confirmation_one blocks=1 confirmed=1 vote_batches=2 vote_processed_events=2 nonfinal_vote_hashes=[1] nonfinal_vote_ok=[1] final_vote_hashes=[1] final_vote_ok=[1] dequeue_us=6 process_us=361 schedule_us=86 nonfinal_vote_to_quorum_us=7602 final_vote_to_election_confirmed_us=6198 aec_fact_process_us=157 cement_us=1317 slowest_stage=nonfinal_vote_to_quorum
```

Observed local 256-block run:

```text
full_local_confirmation_256 blocks=256 confirmed=256 vote_batches=4 vote_processed_events=4 nonfinal_vote_hashes=[7, 249] nonfinal_vote_ok=[7, 249] final_vote_hashes=[8, 248] final_vote_ok=[8, 248] dequeue_us=127 process_us=76634 schedule_us=11877 nonfinal_vote_to_quorum_us=18388 final_vote_to_election_confirmed_us=20958 aec_fact_process_us=3913 cement_us=18478 slowest_stage=process_batch
```

KPI score for this slice:

```text
backlog visibility       partial: this measures fixed one-block and one-batch service time, not repeated backlog growth
throughput visibility    yes: one-block and 256-block batch timings include accepted and confirmed counts
latency visibility       yes: per-stage elapsed time is reported through voting and cementing
source fairness          no: intentionally single-source BlockSource::Live workload
priority fidelity        yes: accepted blocks pass through PriorityScheduler and recomputed ledger BlockPriority
boundary realism         yes: real LMDB processing and confirmation-height writes; real vote generation/processing workers
determinism              partial: no peer network timing, but worker scheduling and local timing are runtime-dependent
actionability            yes: 256-block batch still points to Ledger::process_batch; one-block latency points to vote-generation/processing delay
```

Current full-confirmation conclusion:

```text
For a single block, the largest local latency is final_vote_to_election_confirmed.
For one 256-block batch, the largest local latency remains Ledger::process_batch.

This refines the earlier conclusion:
├─ batch throughput pressure: Ledger::process_batch remains the top measured stage
└─ single-block confirmation latency: local vote generation/processing delay is larger than ledger insertion

This still excludes real peer network transport and remote representative behavior.
```

## 2026-06-11 AecFactProcessor Backpressure Dispatch Timing

Implemented an ignored 10k pressure benchmark in `node/src/consensus/election_schedulers/priority/priority_scheduler.rs`.

```text
AEC fact processor dispatch slice
├─ workload
│  ├─ 10 logical live peers
│  ├─ 1,000 valid live blocks per peer
│  └─ 10,000 real saved live blocks converted into AecFact::ElectionConfirmed
├─ production path used
│  ├─ BlockProcessorQueue::push with default live peer capacity
│  ├─ FairQueue origins split by (BlockSource::Live, ChannelId)
│  ├─ BlockProcessorQueue::pop_blocking
│  ├─ real LMDB-backed Ledger::process_batch
│  ├─ backpressure_channel
│  ├─ spawn_backpressure_processor
│  ├─ AecFactProcessor::process
│  ├─ ConfirmingSet::add
│  ├─ ConfirmingSet confirmation-height thread
│  └─ real LMDB-backed Ledger::confirm_batch
├─ intentionally excluded
│  ├─ election scheduling
│  ├─ vote generation
│  └─ remote peer network transport
└─ command
   └─ cargo test -p rsnano_node pressure_aec_fact_processor_thread_dispatch_reports_10k_across_10_peers_timing -- --ignored --nocapture
      └─ result: 1 passed, 0 failed
```

Observed local run:

```text
aec_fact_thread_dispatch_10k_10_peers blocks=10000 peers=10 confirmed=10000 dequeue_us=7507 process_us=3026750 aec_fact_thread_dispatch_us=40781 cement_us=664219 slowest_stage=process_batch
```

KPI score for this slice:

```text
backlog visibility       partial: waits for all facts to reach ConfirmingSet, but does not chart queue depth over time
throughput visibility    yes: reports 10k dispatch, processing, and cementing elapsed times
latency visibility       yes: reports dispatch-to-ConfirmingSet and cementing elapsed time
source fairness          partial: uses 10 live peer origins, but no cross-source Live/Bootstrap/Unchecked competition
priority fidelity        no: confirmed-election dispatch is after priority/election selection
boundary realism         yes: real backpressure thread, real AecFactProcessor, real ConfirmingSet, real LMDB writes
determinism              partial: deterministic data, but thread scheduling and local timing are runtime-dependent
actionability            yes: isolates AecFactProcessor/backpressure handoff from vote generation and scheduling
```

Current dispatch conclusion:

```text
In the isolated 10k confirmed-election dispatch workload, the AecFactProcessor backpressure handoff
is not the first measured limiter:
├─ process_batch: 3,026,750 us
├─ aec_fact_thread_dispatch: 40,781 us
└─ cementing: 664,219 us

This does not rule out AecFactProcessor lock interplay under a mixed real node workload with concurrent vote
processing, scheduler notifications, winner broadcasts, and bootstrap/fork events. It does rule out the simple
10-peer / 10k ElectionConfirmed handoff as the top local limiter in this benchmark.
```

# Current Evidence Report

This report identifies the first measured local limiter for the named deterministic workload.

```text
measured / characterized stages
├─ ProcessQueue
│  ├─ evidence type: deterministic behavior characterization
│  ├─ backlog/latency evidence: Live dequeue positions under competing source backlog
│  └─ current pressure finding: source shares can create pre-ledger live delay
├─ Ledger::process_batch
│  ├─ evidence type: nullable behavior + real LMDB timing
│  ├─ ledger-only local timing: 64 blocks, 19206 us, 3332.23 blocks/sec
│  ├─ comparable-pipeline timing: 64 blocks, 19805 us
│  ├─ sustained-pressure timing: 192 blocks, 58283 us total
│  └─ current pressure finding: slowest measured serviced stage in the sustained live-synced pressure slice
├─ PriorityScheduler / PriorityBuckets
│  ├─ evidence type: deterministic behavior characterization
│  ├─ backlog/latency evidence: bucket length and candidate retention
│  └─ current pressure finding: recomputed ledger priority and dependency checks control activation
├─ AEC
│  ├─ evidence type: deterministic behavior + local vote/quorum timing
│  ├─ backlog/latency evidence: retained queued candidate when too weak
│  └─ current pressure finding: full local vote path confirms one block and 256 blocks without peer transport
├─ AecFactProcessor
│  ├─ evidence type: full local direct processing + ignored 10k backpressure-thread timing
│  ├─ direct full-local timing: 256 blocks, 3913 us cumulative fact processing
│  ├─ isolated backpressure timing: 10 peers / 10k ElectionConfirmed facts, 40781 us to reach ConfirmingSet
│  └─ current pressure finding: not the first measured limiter for simple confirmed-election handoff
└─ Ledger::confirm_batch / ConfirmingSet
   ├─ evidence type: deterministic behavior + real LMDB timing + ConfirmingSet thread timing
   ├─ ledger-only local timing: 64 blocks, 3370 us, 18988.52 blocks/sec
   ├─ comparable-pipeline timing: 64 blocks, 3519 us
   ├─ sustained-pressure timing: 192 blocks, 10751 us total
   ├─ full-local one-block cementing: 1324 us
   ├─ full-local 256-block cementing: 18478 us
   ├─ isolated 10-peer / 10k AecFactProcessor-driven cementing: 664219 us
   └─ current pressure finding: ledger cementing remains faster than ledger insertion in the 256-block local measured workload
```

Current responsible conclusion:

```text
The first measured local throughput limiter in the named fully synced valid-live pressure workload is
Ledger::process_batch at the real LMDB boundary.

The first growing backlog is observable in BlockProcessorQueue before ledger processing when arrivals exceed
the serviced batch rate. The owner to optimize is still Ledger::process_batch, because queue dequeue is fast,
accepted blocks schedule/confirm faster than insertion in this workload, and the queue grows immediately upstream
of ledger processing.

This limits block acceptance directly and confirmation indirectly. The full local confirmation slice now includes
local vote generation, vote processing, quorum transition, ConfirmingSet, and LMDB confirmation-height writes.
It is not evidence that real peer network admission, remote representative behavior, bootstrap catch-up, fork storms,
or gap storms have the same bottleneck.

Recommended next engineering goal:
├─ owner: Ledger::process_batch / ledger storage insertion path
├─ question: split validation time, write transaction time, and commit/index update time for valid live blocks
├─ reason: process_batch is the first measured limiter, but its internal sub-stage is not yet isolated
└─ constraint: keep measurement at the ledger boundary; do not add duplicate queue or confirmation ownership
```
