use std::{
    collections::{HashSet, VecDeque},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering, Ordering::Relaxed},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use crate::ledger_factory::default_ledger_store_factory;
use rsnano_ledger::{CementingObserver, Ledger};
use rsnano_types::{BlockHash, SavedBlock};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{DetailType, StatType, Stats},
    sync::backpressure_channel::Sender,
    thread_pool::ThreadPool,
};

use super::ordered_entries::OrderedEntries;
use crate::{
    block_processing::{LedgerEvent, ProcessedResult},
    consensus::{ConfirmedElectionsCache, election::ConfirmedElection},
};

/// A block that is currently cementing
#[derive(Clone)]
pub struct CementingEntry {
    pub confirmation_root: BlockHash,
    pub timestamp: Instant,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmingSetConfig {
    pub batch_size: usize,
    /// Maximum number of dependent blocks to be stored in memory during processing
    pub max_blocks: usize,
    pub max_queued_notifications: usize,

    /// Maximum number of failed blocks to wait for requeuing
    pub max_deferred: usize,
    /// Max age of deferred blocks before they are dropped
    pub deferred_age_cutoff: Duration,
}

impl Default for ConfirmingSetConfig {
    fn default() -> Self {
        Self {
            batch_size: 256,
            max_blocks: 16 * 1024,
            max_queued_notifications: 8,
            max_deferred: 16 * 1024,
            deferred_age_cutoff: Duration::from_secs(15 * 60),
        }
    }
}

/// Set of blocks to be durably confirmed
pub struct ConfirmingSet {
    thread: Arc<ConfirmingSetThread>,
    join_handle: Mutex<Option<JoinHandle<()>>>,
    writer_stats: Arc<ConfirmationHeightWriterStats>,
}

impl ConfirmingSet {
    pub fn new(config: ConfirmingSetConfig, ledger: Arc<Ledger>, stats: Arc<Stats>) -> Self {
        let writer_stats = Arc::new(ConfirmationHeightWriterStats::default());
        Self {
            join_handle: Mutex::new(None),
            thread: Arc::new(ConfirmingSetThread {
                mutex: Mutex::new(ConfirmingSetImpl {
                    set: OrderedEntries::default(),
                    deferred: OrderedEntries::default(),
                    current: HashSet::new(),
                    stats: stats.clone(),
                    config: config.clone(),
                    near_full: false,
                    cool_down: false,
                    near_full_limit: config.max_blocks * 100 / 75,
                    recovered_limit: config.max_blocks * 100 / 50,
                    election_cache: ConfirmedElectionsCache::default(),
                }),
                writer_stats: writer_stats.clone(),
                stopped: AtomicBool::new(false),
                condition: Condvar::new(),
                ledger,
                stats,
                config,
                workers: ThreadPool::new(1, "Conf notif"),
                event_publisher: Mutex::new(None),
            }),
            writer_stats,
        }
    }

    pub fn new_null() -> Self {
        Self::new(
            ConfirmingSetConfig::default(),
            Arc::new(Ledger::new_null(default_ledger_store_factory())),
            Arc::new(Stats::default()),
        )
    }

    pub fn set_event_publisher(&self, sink: Sender<LedgerEvent>) {
        *self.thread.event_publisher.lock().unwrap() = Some(sink);
    }

    /// Adds a block to the set of blocks to be confirmed
    pub fn add_block(&self, hash: BlockHash) {
        self.thread.add(hash, None);
    }

    /// Adds a block + its election to the set of blocks to be confirmed
    pub fn add(&self, election: ConfirmedElection) {
        self.thread.add(election.winner.hash(), Some(election));
    }

    pub fn writer_stats(&self) -> Arc<ConfirmationHeightWriterStats> {
        Arc::clone(&self.writer_stats)
    }

    pub fn start(&self) {
        debug_assert!(self.join_handle.lock().unwrap().is_none());

        let thread = Arc::clone(&self.thread);
        *self.join_handle.lock().unwrap() = Some(
            std::thread::Builder::new()
                .name("Conf height".to_string())
                .spawn(move || thread.run())
                .unwrap(),
        );
    }

    pub fn stop(&self) {
        self.thread.stop();
        let handle = self.join_handle.lock().unwrap().take();
        if let Some(handle) = handle {
            handle.join().unwrap();
        }
        self.thread.workers.join();
    }

    /// Added blocks will remain in this set until after ledger has them marked as confirmed.
    pub fn contains(&self, hash: &BlockHash) -> bool {
        self.thread.contains(hash)
    }

    pub fn len(&self) -> usize {
        self.thread.len()
    }

    pub fn info(&self) -> ConfirmingSetInfo {
        let guard = self.thread.mutex.lock().unwrap();
        ConfirmingSetInfo {
            size: guard.set.len(),
            max_size: self.thread.config.max_blocks,
        }
    }

    /// Requeue blocks that failed to cement immediately due to missing ledger blocks
    pub fn requeue_blocks(&self, batch: &[ProcessedResult]) {
        let mut should_notify = false;
        {
            let mut guard = self.thread.mutex.lock().unwrap();
            for result in batch {
                if let Some(entry) = guard.deferred.remove(&result.block.hash()) {
                    self.thread
                        .stats
                        .inc(StatType::ConfirmingSet, DetailType::Requeued);
                    guard.set.push_back(entry);
                    should_notify = true;
                }
            }
        }

        if should_notify {
            self.thread.condition.notify_all();
        }
    }

    pub(crate) fn do_election_cache(&self, mut action: impl FnMut(&ConfirmedElectionsCache)) {
        let guard = self.thread.mutex.lock().unwrap();
        action(&guard.election_cache);
    }

    pub fn set_cooldown(&self, cool_down: bool) {
        self.thread.mutex.lock().unwrap().cool_down = cool_down;
        self.thread.condition.notify_all();
    }
}

impl ContainerInfoProvider for ConfirmingSet {
    fn container_info(&self) -> ContainerInfo {
        let guard = self.thread.mutex.lock().unwrap();
        [
            ("set", guard.set.len(), 0),
            ("deferred", guard.deferred.len(), 0),
        ]
        .into()
    }
}

#[derive(Default)]
pub struct ConfirmingSetInfo {
    pub size: usize,
    pub max_size: usize,
}

impl Drop for ConfirmingSet {
    fn drop(&mut self) {
        self.stop();
    }
}

struct ConfirmingSetThread {
    mutex: Mutex<ConfirmingSetImpl>,
    stopped: AtomicBool,
    condition: Condvar,
    ledger: Arc<Ledger>,
    stats: Arc<Stats>,
    config: ConfirmingSetConfig,
    writer_stats: Arc<ConfirmationHeightWriterStats>,
    workers: ThreadPool,
    event_publisher: Mutex<Option<Sender<LedgerEvent>>>,
}

impl ConfirmingSetThread {
    fn stop(&self) {
        {
            let _guard = self.mutex.lock().unwrap();
            self.stopped.store(true, Ordering::SeqCst);
        }
        drop(self.event_publisher.lock().unwrap().take());
        self.condition.notify_all();
    }

    fn add(&self, hash: BlockHash, election: Option<ConfirmedElection>) {
        let added;
        let mut near_full_warning = false;
        {
            let mut guard = self.mutex.lock().unwrap();
            if let Some(e) = election {
                guard.election_cache.insert(e);
            }
            added = guard.set.push_back(CementingEntry {
                confirmation_root: hash,
                timestamp: Instant::now(),
            });

            if !guard.near_full && guard.set.len() + guard.current.len() >= guard.near_full_limit {
                guard.near_full = true;
                near_full_warning = true;
            }
        };

        if added {
            self.condition.notify_all();
            self.stats.inc(StatType::ConfirmingSet, DetailType::Insert);
        } else {
            self.stats
                .inc(StatType::ConfirmingSet, DetailType::Duplicate);
        }

        if near_full_warning {
            self.notify(LedgerEvent::ConfirmingSetNearFull);
        }
    }

    fn contains(&self, hash: &BlockHash) -> bool {
        let guard = self.mutex.lock().unwrap();
        guard.set.contains(hash) || guard.deferred.contains(hash) || guard.current.contains(hash)
    }

    fn len(&self) -> usize {
        // Do not report deferred blocks, as they are not currently being processed (and might never be requeued)
        let guard = self.mutex.lock().unwrap();
        guard.set.len() + guard.current.len()
    }

    fn run(&self) {
        let mut guard = self.mutex.lock().unwrap();
        while !self.stopped.load(Ordering::SeqCst) {
            self.stats.inc(StatType::ConfirmingSet, DetailType::Loop);
            let evicted = guard.cleanup();

            // Notify about evicted blocks so that other components can perform necessary cleanup
            if !evicted.is_empty() {
                drop(guard);
                {
                    for entry in evicted {
                        self.notify(LedgerEvent::ConfirmationFailed(entry.confirmation_root));
                    }
                }
                guard = self.mutex.lock().unwrap();
            }

            if !guard.set.is_empty() {
                let batch = guard.next_batch(self.config.batch_size);

                // Keep track of the blocks we're currently cementing, so that the .contains (...) check is accurate
                debug_assert!(guard.current.is_empty());
                for entry in &batch {
                    guard.current.insert(entry.confirmation_root);
                }
                let recovered = guard.near_full && guard.set.len() < guard.recovered_limit;
                if recovered {
                    guard.near_full = false;
                }

                drop(guard);

                self.run_batch(batch);
                if recovered {
                    self.notify(LedgerEvent::ConfirmingSetRecovered);
                }

                guard = self.mutex.lock().unwrap();
            } else {
                guard = self
                    .condition
                    .wait_while(guard, |i| {
                        (i.set.is_empty() || i.cool_down) && !self.stopped.load(Ordering::SeqCst)
                    })
                    .unwrap();
            }
        }
    }

    fn run_batch(&self, batch: VecDeque<CementingEntry>) {
        let _guard = self.writer_stats.start_optimistic_writer();
        let prev_successes = self.ledger.optimistic_successes();
        let prev_conflicts = self.ledger.optimistic_conflicts();
        let prev_fallbacks = self.ledger.pessimistic_fallbacks();

        let mut notifier = CementedNotifier::new(self);
        self.ledger.confirm_batch(
            batch.iter().map(|i| &i.confirmation_root),
            &self.stopped,
            self.config.max_blocks,
            &mut notifier,
        );

        let optimistic_successes = self.ledger.optimistic_successes();
        let optimistic_conflicts = self.ledger.optimistic_conflicts();
        let pessimistic_fallbacks = self.ledger.pessimistic_fallbacks();

        self.writer_stats.add_deltas(
            optimistic_successes.saturating_sub(prev_successes),
            optimistic_conflicts.saturating_sub(prev_conflicts),
            pessimistic_fallbacks.saturating_sub(prev_fallbacks),
        );

        // Clear current set only after the transaction is committed
        self.mutex.lock().unwrap().current.clear();
    }

    fn notify(&self, event: LedgerEvent) {
        if let Some(sender) = self.event_publisher.lock().unwrap().as_ref() {
            sender.send(event).unwrap();
        }
    }
}

struct ConfirmingSetImpl {
    /// Blocks that are ready to be cemented
    set: OrderedEntries,
    /// Blocks that could not be cemented immediately (e.g. waiting for rollbacks to complete)
    deferred: OrderedEntries,
    /// Blocks that are being cemented in the current batch
    current: HashSet<BlockHash>,

    stats: Arc<Stats>,
    config: ConfirmingSetConfig,
    near_full: bool,
    cool_down: bool,
    near_full_limit: usize,
    recovered_limit: usize,
    election_cache: ConfirmedElectionsCache,
}

impl ConfirmingSetImpl {
    fn next_batch(&mut self, max_count: usize) -> VecDeque<CementingEntry> {
        let mut results = VecDeque::new();
        // TODO: use extract_if once it is stablized
        while let Some(entry) = self.set.pop_front() {
            results.push_back(entry);
            if results.len() >= max_count {
                break;
            }
        }
        results
    }

    fn cleanup(&mut self) -> Vec<CementingEntry> {
        let mut evicted = Vec::new();

        let cutoff = Instant::now() - self.config.deferred_age_cutoff;
        let should_evict = |entry: &CementingEntry| entry.timestamp < cutoff;

        // Iterate in sequenced (insertion) order
        loop {
            let Some(entry) = self.deferred.front() else {
                break;
            };

            if should_evict(entry) || self.deferred.len() > self.config.max_deferred {
                self.stats.inc(StatType::ConfirmingSet, DetailType::Evicted);
                let entry = self.deferred.pop_front().unwrap();
                evicted.push(entry);
            } else {
                // Entries are sequenced, so we can stop here and avoid unnecessary iteration
                break;
            }
        }
        evicted
    }
}

#[derive(Default)]
pub struct ConfirmationHeightWriterStats {
    optimistic_successes: AtomicU64,
    optimistic_conflicts: AtomicU64,
    pessimistic_fallbacks: AtomicU64,
    optimistic_active: AtomicU64,
    max_optimistic_concurrency: AtomicU64,
}

impl ConfirmationHeightWriterStats {
    pub fn optimistic_successes(&self) -> u64 {
        self.optimistic_successes.load(Relaxed)
    }

    pub fn optimistic_conflicts(&self) -> u64 {
        self.optimistic_conflicts.load(Relaxed)
    }

    pub fn pessimistic_fallbacks(&self) -> u64 {
        self.pessimistic_fallbacks.load(Relaxed)
    }

    pub fn max_optimistic_concurrency(&self) -> u64 {
        self.max_optimistic_concurrency.load(Relaxed)
    }

    fn start_optimistic_writer(&self) -> OptimisticWriterGuard<'_> {
        let active = self.optimistic_active.fetch_add(1, Relaxed) + 1;
        let mut observed = self.max_optimistic_concurrency.load(Relaxed);
        while active > observed {
            match self
                .max_optimistic_concurrency
                .compare_exchange(observed, active, Relaxed, Relaxed)
            {
                Ok(_) => break,
                Err(actual) => observed = actual,
            }
        }
        OptimisticWriterGuard { stats: self }
    }

    fn end_optimistic_writer(&self) {
        self.optimistic_active.fetch_sub(1, Relaxed);
    }

    fn add_deltas(&self, successes: u64, conflicts: u64, fallbacks: u64) {
        self.optimistic_successes.fetch_add(successes, Relaxed);
        self.optimistic_conflicts.fetch_add(conflicts, Relaxed);
        self.pessimistic_fallbacks.fetch_add(fallbacks, Relaxed);
    }
}

struct OptimisticWriterGuard<'a> {
    stats: &'a ConfirmationHeightWriterStats,
}

impl Drop for OptimisticWriterGuard<'_> {
    fn drop(&mut self) {
        self.stats.end_optimistic_writer();
    }
}

pub struct ConfirmationContext {
    /// The block that was confirmed
    pub block: SavedBlock,
    /// The hash of the block which caused the block to be cemented
    pub confirmation_root: BlockHash,
}

struct CementedNotifier<'a> {
    confirming_set: &'a ConfirmingSetThread,
    already_confirmed: VecDeque<BlockHash>,
}

impl<'a> CementedNotifier<'a> {
    fn new(confirming_set: &'a ConfirmingSetThread) -> Self {
        Self {
            confirming_set,
            already_confirmed: Default::default(),
        }
    }
}

impl<'a> CementingObserver for CementedNotifier<'a> {
    fn already_confirmed(&mut self, hash: &BlockHash) {
        self.already_confirmed.push_back(*hash);
    }

    fn cementing_failed(&mut self, hash: &BlockHash) {
        self.confirming_set
            .mutex
            .lock()
            .unwrap()
            .deferred
            .push_back(CementingEntry {
                confirmation_root: *hash,
                timestamp: Instant::now(),
            });
    }

    fn batch_confirmed(&mut self, batch: Vec<(SavedBlock, BlockHash)>) {
        self.confirming_set
            .notify(LedgerEvent::BlocksConfirmed(batch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Arc, Barrier},
        time::{Duration, Instant},
    };
    use rsnano_ledger::{LedgerSet, test_helpers::SavedBlockLatticeBuilder};
    use rsnano_types::{Amount, Block, PrivateKey, WorkNonce};
    use crate::ledger_factory::default_ledger_store_factory;

    #[test]
    fn add_exists() {
        let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));
        let confirming_set =
            ConfirmingSet::new(Default::default(), ledger, Arc::new(Stats::default()));
        let hash = BlockHash::from(1);
        confirming_set.add_block(hash);
        assert!(confirming_set.contains(&hash));
    }

    #[test]
    fn confirmation_height_runs_concurrently_with_ledger_writes() {
        let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));
        let stats = Arc::new(Stats::default());
        let confirming_set = ConfirmingSet::new(
            ConfirmingSetConfig {
                batch_size: 1,
                ..Default::default()
            },
            ledger.clone(),
            stats,
        );
        confirming_set.start();

        let mut lattice = SavedBlockLatticeBuilder::with_stub_work();
        let key1 = PrivateKey::new();
        let key2 = PrivateKey::from(2);

        let send1 = lattice.genesis().send(&key1, Amount::raw(1));
        let mut send1_block: Block = send1.clone().into();
        send1_block.set_work(WorkNonce::new(u64::MAX));
        ledger.process_one(&send1_block).unwrap();

        let send2 = lattice.genesis().send(&key2, Amount::raw(1));
        let mut send2_block: Block = send2.clone().into();
        send2_block.set_work(WorkNonce::new(u64::MAX));

        let barrier = Arc::new(Barrier::new(2));
        let barrier_clone = barrier.clone();
        let ledger_clone = ledger.clone();
        let handle = std::thread::spawn(move || {
            barrier_clone.wait();
            ledger_clone.process_one(&send2_block).unwrap();
        });

        barrier.wait();
        confirming_set.add_block(send1.hash());

        let deadline = Instant::now() + Duration::from_secs(5);
        while !ledger.confirmed().block_exists(&send1.hash()) {
            assert!(Instant::now() < deadline, "confirmation height processing stalled");
            std::thread::sleep(Duration::from_millis(10));
        }

        handle.join().unwrap();
        confirming_set.stop();

        let writer_stats = confirming_set.writer_stats();
        assert!(writer_stats.optimistic_successes() >= 1);
        assert_eq!(writer_stats.optimistic_conflicts(), 0);
        assert_eq!(writer_stats.pessimistic_fallbacks(), 0);
        assert!(writer_stats.max_optimistic_concurrency() >= 1);
    }
}
