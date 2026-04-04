# Optimistic Scheduler

## Purpose

The optimistic scheduler proactively promotes accounts with a large **confirmation gap** — the difference between an account's total block count and its confirmed height — into the Active Elections Container (AEC).

In the Nano consensus protocol, a block is only confirmed once an election is held for it. Accounts that accumulate many unconfirmed blocks (e.g. after a node restart or during heavy network load) would otherwise stall. The optimistic scheduler detects these accounts during backlog scanning and schedules elections for their head blocks, without waiting for an explicit prioritization request.

The word *optimistic* reflects the strategy: the scheduler bets that accounts with large gaps are worth confirming now, even speculatively, to clear the backlog faster.

## Design

The module follows the **A-frame architecture** used throughout the codebase:

- **Logic** (`OptimisticSchedulerLogic`) — pure computation, no I/O. Decides which accounts qualify, manages the candidate queue, and enforces capacity limits.
- **Application owner** (`CandidateCoordinator`) — owns the shared scheduler loop, reads from the `Ledger`, writes through `AecService`, and consults the `ConfirmingSet`.
- **Facade** (`ElectionSchedulers`) — receives backlog signals and forwards optimistic admission into the coordinator.
- The application layer calls into the logic layer to make all optimistic scheduling decisions (the "Logic Sandwich" pattern).

### Components

| Component | Role |
|-----------|------|
| `ElectionSchedulers` | Source-agnostic facade. Receives backlog activation signals. |
| `CandidateCoordinator` | Application. Owns the shared loop, optimistic wakeups, ledger access, and AEC insertion timing. |
| `OptimisticSchedulerLogic` | Pure logic. Gate-keeps activation, manages the candidate queue. |
| `CandidateQueue` | Dual-indexed data structure. Supports O(log n) pop-by-highest-gap and O(1) account lookup. |
| `OptimisticSchedulerParams` | Configuration (gap threshold, capacity, election cap, activation delay). |
| `OptimisticSchedulerStats` | Atomic telemetry counters exposed via `StatsSource`. |

### Activation flow

1. The backlog scan calls `ElectionSchedulers::activate_backlog(...)`.
2. `ElectionSchedulers` forwards the optimistic half of that signal to `CandidateCoordinator::activate_optimistic(account, block_count, confirmation_height)`.
3. `OptimisticSchedulerLogic::try_activate` computes `gap = block_count − confirmation_height`.
4. If `gap < gap_threshold` the account is rejected. If the queue is full, the account must have a strictly higher gap than the current minimum to evict it; otherwise it is rejected.
5. Accepted accounts are enqueued in `CandidateQueue` with their insertion timestamp.

### Coordinator scheduling path

1. The shared coordinator loop wakes when there is AEC vacancy and an optimistic candidate is old enough (older than `activation_delay`).
2. `CandidateCoordinator::run_optimistic()` pops candidates in descending gap order through `OptimisticSchedulerLogic` and `CandidateQueue`.
3. For each account the coordinator looks up the head block in the ledger, checks it is not already confirmed, and inserts it through `AecService::insert(AecInsertRequest::new_optimistic(...))`.
4. The optimistic path caps optimistic elections at `max_elections` and respects the overall AEC vacancy alongside the coordinator's other sources.

### CandidateQueue internals

The queue maintains two parallel indices:

- `by_account: HashMap<Account, u64>` — O(1) existence check and gap lookup.
- `by_gap: BTreeMap<u64, Vec<(Account, Timestamp)>>` — ordered by gap, enabling O(log n) highest-gap pop and O(log n) lowest-gap eviction.

When an account is re-activated with a new gap its entry is moved to the correct bucket while **preserving the original insertion timestamp**, so `activation_delay` is not inadvertently reset.

## Class Diagram

```mermaid
classDiagram
    class ElectionSchedulers {
        +activate_backlog(any, account, account_info, conf_info)
    }

    class CandidateCoordinator {
        -clock: Arc~SteadyClock~
        +activate_optimistic(account, block_count, conf_height) bool
        +notify()
        +start_loop()
        +stop()
        -run()
        -run_optimistic()
        -run_one_optimistic(account)
    }

    class OptimisticSchedulerLogic {
        +try_activate(account, block_count, conf_height, now) bool
        +pop_candidate(now) Option~Account~
        +has_ready_candidate(now) bool
        +next_activation_delay(now) Option~Duration~
        +max_elections() usize
    }

    class OptimisticSchedulerParams {
        +gap_threshold: u64
        +max_candidates: usize
        +max_elections: usize
        +activation_delay: Duration
    }

    class OptimisticSchedulerStats {
        +loop_count: AtomicU64
        +activated_count: AtomicU64
        +insert_count: AtomicU64
        +insert_failed_count: AtomicU64
    }

    class CandidateQueue {
        +insert(account, now, gap)
        +pop_first(cutoff) Option~Account~
        +pop_lowest_gap_entry() Option~Account~
        +has_candidate(cutoff) bool
        +contains(account) bool
        +min_gap() Option~u64~
        +len() usize
    }

    class AecService {
        +insert(request, now)
    }

    class Ledger {
    }

    class ConfirmingSet {
    }

    ElectionSchedulers --> CandidateCoordinator : forwards backlog activation
    CandidateCoordinator *-- OptimisticSchedulerLogic : owns optimistic state
    CandidateCoordinator *-- OptimisticSchedulerStats : owns
    CandidateCoordinator --> AecService : inserts optimistic elections
    CandidateCoordinator --> Ledger : reads head blocks
    CandidateCoordinator --> ConfirmingSet : checks confirmation status
    OptimisticSchedulerLogic *-- CandidateQueue : owns
    OptimisticSchedulerLogic *-- OptimisticSchedulerParams : owns
```
