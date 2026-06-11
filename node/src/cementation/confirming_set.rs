use std::{
    collections::{HashSet, VecDeque},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use rsnano_ledger::{CementingObserver, Ledger, ProcessResult};
use rsnano_types::{BlockHash, SavedBlock};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{DetailType, StatType, Stats},
    thread_pool::ThreadPool,
};

use super::ordered_entries::OrderedEntries;
use crate::{
    cementation::ConfirmingSetEvent,
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
            deferred_age_cutoff: Duration::from_mins(15),
        }
    }
}

/// Set of blocks to be durably confirmed
pub struct ConfirmingSet {
    thread: Arc<ConfirmingSetThread>,
    join_handle: Mutex<Option<JoinHandle<()>>>,
}

impl ConfirmingSet {
    pub fn new(config: ConfirmingSetConfig, ledger: Arc<Ledger>, stats: Arc<Stats>) -> Self {
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
                stopped: AtomicBool::new(false),
                condition: Condvar::new(),
                ledger,
                stats,
                config,
                workers: ThreadPool::new(1, "Conf notif"),
                event_publisher: Mutex::new(None),
            }),
        }
    }

    pub fn new_null() -> Self {
        Self::new(
            ConfirmingSetConfig::default(),
            Arc::new(Ledger::new_null()),
            Arc::new(Stats::default()),
        )
    }

    pub fn set_event_publisher<F>(&self, sink: F)
    where
        F: Fn(ConfirmingSetEvent) + Send + 'static,
    {
        *self.thread.event_publisher.lock().unwrap() = Some(Box::new(sink));
    }

    /// Adds a block to the set of blocks to be confirmed
    pub fn add_block(&self, hash: BlockHash) {
        self.thread.add(hash, None);
    }

    /// Adds a block + its election to the set of blocks to be confirmed
    pub fn add(&self, election: ConfirmedElection) {
        self.thread.add(election.winner.hash(), Some(election));
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

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn info(&self) -> ConfirmingSetInfo {
        let guard = self.thread.mutex.lock().unwrap();
        ConfirmingSetInfo {
            size: guard.set.len(),
            max_size: self.thread.config.max_blocks,
        }
    }

    /// Requeue blocks that failed to cement immediately due to missing ledger blocks
    pub fn requeue_blocks(&self, batch: &[ProcessResult]) {
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
    workers: ThreadPool,
    event_publisher: Mutex<Option<Box<dyn Fn(ConfirmingSetEvent) + Send>>>,
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
            self.notify(ConfirmingSetEvent::NearFull);
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
                        self.notify(ConfirmingSetEvent::ConfirmationFailed(
                            entry.confirmation_root,
                        ));
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
                    self.notify(ConfirmingSetEvent::Recovered);
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
        let mut notifier = CementedNotifier::new(self);
        self.ledger.confirm_batch(
            batch.iter().map(|i| &i.confirmation_root),
            &self.stopped,
            self.config.max_blocks,
            &mut notifier,
        );

        // Clear current set only after the transaction is committed
        self.mutex.lock().unwrap().current.clear();
    }

    fn notify(&self, event: ConfirmingSetEvent) {
        if let Some(publisher) = self.event_publisher.lock().unwrap().as_ref() {
            publisher(event);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_ledger::{LedgerSet, test_helpers::UnsavedBlockLatticeBuilder};
    use rsnano_types::PrivateKey;

    #[test]
    fn add_exists() {
        let ledger = Arc::new(Ledger::new_null());
        let confirming_set =
            ConfirmingSet::new(Default::default(), ledger, Arc::new(Stats::default()));
        let hash = BlockHash::from(1);
        confirming_set.add_block(hash);
        assert!(confirming_set.contains(&hash));
    }

    #[test]
    fn pressure_run_batch_confirms_queued_block_and_clears_current_set() {
        let ledger = Arc::new(Ledger::new_null());
        let confirming_set = create_confirming_set(ledger.clone(), 16);
        let block = process_unconfirmed_receive(&ledger, PrivateKey::from(1), 1);
        let mut batch = VecDeque::new();
        batch.push_back(CementingEntry {
            confirmation_root: block.hash(),
            timestamp: Instant::now(),
        });

        confirming_set
            .thread
            .mutex
            .lock()
            .unwrap()
            .current
            .insert(block.hash());

        confirming_set.thread.run_batch(batch);

        assert!(ledger.confirmed().block_exists(&block.hash()));
        assert!(
            confirming_set
                .thread
                .mutex
                .lock()
                .unwrap()
                .current
                .is_empty()
        );
    }

    #[test]
    fn pressure_run_batch_defers_missing_confirmation_root() {
        let ledger = Arc::new(Ledger::new_null());
        let confirming_set = create_confirming_set(ledger, 16);
        let missing = BlockHash::from(999);
        let mut batch = VecDeque::new();
        batch.push_back(CementingEntry {
            confirmation_root: missing,
            timestamp: Instant::now(),
        });

        confirming_set
            .thread
            .mutex
            .lock()
            .unwrap()
            .current
            .insert(missing);

        confirming_set.thread.run_batch(batch);

        let guard = confirming_set.thread.mutex.lock().unwrap();
        assert!(guard.current.is_empty());
        assert!(guard.deferred.contains(&missing));
    }

    /* Test helpers */

    fn create_confirming_set(ledger: Arc<Ledger>, max_blocks: usize) -> ConfirmingSet {
        ConfirmingSet::new(
            ConfirmingSetConfig {
                max_blocks,
                ..Default::default()
            },
            ledger,
            Arc::new(Stats::default()),
        )
    }

    fn process_unconfirmed_receive(
        ledger: &Ledger,
        account: PrivateKey,
        amount: impl Into<rsnano_types::Amount>,
    ) -> SavedBlock {
        let mut lattice = UnsavedBlockLatticeBuilder::with_stub_work();
        let send = lattice.genesis().send(&account, amount);
        let receive = lattice.account(&account).receive(&send);

        ledger.process_one(&send).unwrap();
        ledger.confirm(send.hash());
        ledger.process_one(&receive).unwrap()
    }
}
