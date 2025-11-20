use std::{
    cmp::min,
    sync::{
        Arc, Condvar, Mutex, RwLock,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    thread::JoinHandle,
    time::Duration,
};

use tracing::warn;

use crate::ledger_factory::default_ledger_store_factory;
use rsnano_ledger::{AnySet, Ledger, LedgerSet, OwningAnySet};
use rsnano_network::token_bucket::TokenBucket;
use rsnano_nullable_clock::SteadyClock;
use rsnano_types::{Account, AccountInfo, BlockHash, ConfirmationHeightInfo, SavedBlock};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{DetailType, StatType, Stats},
    sync::backpressure_channel::{Sender, channel},
};
use store_traits::ledger::LedgerStoreFactory;

use super::{
    LedgerEvent, ProcessedResult,
    backlog_index::{BacklogEntry, BacklogIndex},
    backlog_scan::UnconfirmedInfo,
};
use crate::consensus::election_schedulers::priority::{prio_bucket_count, prio_bucket_index};

#[derive(Clone, Debug, PartialEq)]
pub struct BoundedBacklogConfig {
    pub max_backlog: u64,
    pub batch_size: usize,
    pub scan_rate: usize,
}

impl Default for BoundedBacklogConfig {
    fn default() -> Self {
        Self {
            max_backlog: 100_000,
            batch_size: 32,
            scan_rate: 64,
        }
    }
}

pub struct BoundedBacklog {
    thread: Mutex<Option<JoinHandle<()>>>,
    scan_thread: Mutex<Option<JoinHandle<()>>>,
    backlog_impl: Arc<BoundedBacklogImpl>,
}

impl BoundedBacklog {
    pub(crate) fn new(
        config: BoundedBacklogConfig,
        ledger: Arc<Ledger>,
        stats: Arc<Stats>,
        clock: Arc<SteadyClock>,
        publish_event: Sender<LedgerEvent>,
    ) -> Self {
        let backlog_impl = Arc::new(BoundedBacklogImpl {
            condition: Condvar::new(),
            mutex: Mutex::new(BacklogData {
                stopped: false,
                cool_down: false,
                index: BacklogIndex::new(prio_bucket_count()),
                ledger: ledger.clone(),
                config: config.clone(),
                bucket_count: prio_bucket_count(),
                scan_limiter: Mutex::new(TokenBucket::new(config.scan_rate)),
            }),
            config,
            stats,
            ledger,
            clock,
            can_roll_back: RwLock::new(Box::new(|_| true)),
            publish_event: Mutex::new(Some(publish_event)),
            writer_stats: Arc::new(BoundedBacklogWriterStats::default()),
        });

        Self {
            backlog_impl,
            thread: Mutex::new(None),
            scan_thread: Mutex::new(None),
        }
    }

    pub fn new_null() -> Self {
        Self::new_null_with_factory(default_ledger_store_factory())
    }

    pub fn new_null_with_factory(store_factory: Arc<dyn LedgerStoreFactory>) -> Self {
        let config = BoundedBacklogConfig::default();
        let ledger = Arc::new(Ledger::new_null(store_factory));
        let stats = Arc::new(Stats::default());
        let clock = Arc::new(SteadyClock::new_null());
        let (sender, _) = channel(0);

        Self::new(config, ledger, stats, clock, sender)
    }

    pub fn start(&self) {
        debug_assert!(self.thread.lock().unwrap().is_none());

        let backlog_impl = self.backlog_impl.clone();
        let handle = std::thread::Builder::new()
            .name("Bounded backlog".to_owned())
            .spawn(move || backlog_impl.run())
            .unwrap();
        *self.thread.lock().unwrap() = Some(handle);

        let backlog_impl = self.backlog_impl.clone();
        let handle = std::thread::Builder::new()
            .name("Bounded b scan".to_owned())
            .spawn(move || backlog_impl.run_scan())
            .unwrap();
        *self.scan_thread.lock().unwrap() = Some(handle);
    }

    pub fn stop(&self) {
        self.backlog_impl.mutex.lock().unwrap().stopped = true;
        self.backlog_impl.condition.notify_all();

        let handle = self.thread.lock().unwrap().take();
        if let Some(handle) = handle {
            handle.join().unwrap();
        }

        let handle = self.scan_thread.lock().unwrap().take();
        if let Some(handle) = handle {
            handle.join().unwrap();
        }
        drop(self.backlog_impl.publish_event.lock().unwrap().take());
    }

    pub fn writer_stats(&self) -> Arc<BoundedBacklogWriterStats> {
        Arc::clone(&self.backlog_impl.writer_stats)
    }

    // Give other components a chance to veto a rollback
    pub fn can_roll_back(&self, f: impl Fn(&BlockHash) -> bool + Send + Sync + 'static) {
        *self.backlog_impl.can_roll_back.write().unwrap() = Box::new(f);
    }

    pub fn set_cooldown(&self, cool_down: bool) {
        self.backlog_impl.mutex.lock().unwrap().cool_down = cool_down;
        self.backlog_impl.condition.notify_all();
    }

    pub fn activate_batch(&self, batch: &[UnconfirmedInfo]) {
        let mut any = self.backlog_impl.ledger.any();
        for info in batch {
            self.activate(&mut any, &info.account, &info.account_info, &info.conf_info);
        }
    }

    /// Track unconfirmed blocks
    pub fn insert_processed(&self, batch: &[ProcessedResult]) {
        let any = self.backlog_impl.ledger.any();
        for result in batch {
            if result.status.is_ok() {
                if let Some(block) = &result.saved_block {
                    self.insert(&any, block);
                }
            }
        }
    }

    pub fn erase_accounts(&self, accounts: &[Account]) {
        let mut guard = self.backlog_impl.mutex.lock().unwrap();
        for account in accounts {
            guard.index.erase_account(account);
        }
    }

    pub fn erase_hashes(&self, accounts: impl IntoIterator<Item = BlockHash>) {
        let mut guard = self.backlog_impl.mutex.lock().unwrap();
        for account in accounts.into_iter() {
            guard.index.erase_hash(&account);
        }
    }

    fn contains(&self, hash: &BlockHash) -> bool {
        let guard = self.backlog_impl.mutex.lock().unwrap();
        guard.index.contains(hash)
    }

    fn activate<'a>(
        &'a self,
        any: &mut OwningAnySet<'a>,
        _account: &Account,
        account_info: &AccountInfo,
        conf_info: &ConfirmationHeightInfo,
    ) {
        debug_assert!(conf_info.frontier != account_info.head);

        // Insert blocks into the index starting from the account head block
        let mut block = any.get_block(&account_info.head);

        while let Some(blk) = block {
            // We reached the confirmed frontier, no need to track more blocks
            if blk.hash() == conf_info.frontier {
                break;
            }

            // Check if the block is already in the backlog, avoids unnecessary ledger lookups
            if self.contains(&blk.hash()) {
                break;
            }

            let inserted = self.insert(any, &blk);

            // If the block was not inserted, we already have it in the backlog
            if !inserted {
                break;
            }

            if any.should_refresh() {
                *any = self.backlog_impl.ledger.any();
            }

            block = any.get_block(&blk.previous());
        }
    }

    pub fn insert(&self, any: &impl AnySet, block: &SavedBlock) -> bool {
        let priority = any.block_priority(block);
        let bucket_index = prio_bucket_index(priority.balance);

        self.backlog_impl
            .mutex
            .lock()
            .unwrap()
            .index
            .insert(BacklogEntry {
                hash: block.hash(),
                account: block.account(),
                bucket_index,
                priority: priority.time,
            })
    }

    pub fn remove(&self, confirmed: &Vec<(SavedBlock, BlockHash)>) {
        // Remove confirmed blocks from the backlog
        self.erase_hashes(confirmed.iter().map(|i| i.0.hash()));
    }
}

impl Drop for BoundedBacklog {
    fn drop(&mut self) {
        // Thread must be stopped before destruction
        debug_assert!(self.thread.lock().unwrap().is_none());
        debug_assert!(self.scan_thread.lock().unwrap().is_none());
    }
}

impl ContainerInfoProvider for BoundedBacklog {
    fn container_info(&self) -> ContainerInfo {
        let guard = self.backlog_impl.mutex.lock().unwrap();
        ContainerInfo::builder()
            .leaf("backlog", guard.index.len(), 0)
            .node("index", guard.index.container_info())
            .finish()
    }
}

struct BoundedBacklogImpl {
    mutex: Mutex<BacklogData>,
    condition: Condvar,
    config: BoundedBacklogConfig,
    stats: Arc<Stats>,
    ledger: Arc<Ledger>,
    can_roll_back: RwLock<Box<dyn Fn(&BlockHash) -> bool + Send + Sync>>,
    clock: Arc<SteadyClock>,
    publish_event: Mutex<Option<Sender<LedgerEvent>>>,
    writer_stats: Arc<BoundedBacklogWriterStats>,
}

impl BoundedBacklogImpl {
    fn run(&self) {
        let mut guard = self.mutex.lock().unwrap();
        while !guard.stopped {
            guard = self
                .condition
                .wait_timeout_while(guard, Duration::from_secs(1), |i| {
                    !i.stopped && !i.predicate()
                })
                .unwrap()
                .0;

            if guard.stopped {
                return;
            }

            self.stats.inc(StatType::BoundedBacklog, DetailType::Loop);

            // Calculate the number of targets to rollback
            let backlog = self.ledger.backlog_count();

            let target_count = if backlog > self.config.max_backlog {
                backlog - self.config.max_backlog
            } else {
                0
            };

            let can_roll_back = self.can_roll_back.read().unwrap();
            let targets = guard.gather_targets(
                min(target_count as usize, self.config.batch_size),
                &*can_roll_back,
            );

            if !targets.is_empty() {
                drop(guard);
                self.stats.add(
                    StatType::BoundedBacklog,
                    DetailType::GatheredTargets,
                    targets.len() as u64,
                );

                let processed = self.roll_back(&targets, target_count as usize, &*can_roll_back);
                guard = self.mutex.lock().unwrap();

                // Erase rolled back blocks from the index
                for hash in &processed {
                    guard.index.erase_hash(hash);
                }
            } else {
                // Cooldown, this should not happen in normal operation
                self.stats
                    .inc(StatType::BoundedBacklog, DetailType::NoTargets);
                guard = self
                    .condition
                    .wait_timeout_while(guard, Duration::from_millis(100), |i| !i.stopped)
                    .unwrap()
                    .0;
            }
        }
    }

    fn roll_back(
        &self,
        targets: &[BlockHash],
        max_rollbacks: usize,
        can_roll_back: impl Fn(&BlockHash) -> bool,
    ) -> Vec<BlockHash> {
        let stats = self.writer_stats.clone();
        let _guard = stats.start_optimistic_writer();
        let prev_successes = self.ledger.optimistic_successes();
        let prev_conflicts = self.ledger.optimistic_conflicts();
        let prev_fallbacks = self.ledger.pessimistic_fallbacks();

        let results = self
            .ledger
            .roll_back_batch(targets, max_rollbacks, can_roll_back);

        stats.add_deltas(
            self.ledger.optimistic_successes() - prev_successes,
            self.ledger.optimistic_conflicts() - prev_conflicts,
            self.ledger.pessimistic_fallbacks() - prev_fallbacks,
        );

        let mut processed_hashes = Vec::new();
        for result in results.iter() {
            if !result.rolled_back.is_empty() {
                for h in &result.rolled_back {
                    processed_hashes.push(h.hash());
                }
            } else {
                processed_hashes.push(result.target_hash);
            }
        }

        if let Err(e) = self
            .publish_event
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .send(LedgerEvent::BlocksRolledBack(results))
        {
            warn!("Failed to publish rolled back event: {e:?}")
        }

        processed_hashes
    }

    fn run_scan(&self) {
        let mut guard = self.mutex.lock().unwrap();
        while !guard.stopped {
            let mut last = BlockHash::ZERO;
            while !guard.stopped {
                //	wait
                while !guard
                    .scan_limiter
                    .lock()
                    .unwrap()
                    .try_consume(self.config.batch_size, self.clock.now())
                {
                    guard = self
                        .condition
                        .wait_timeout(guard, Duration::from_millis(100))
                        .unwrap()
                        .0;
                    if guard.stopped {
                        return;
                    }
                }

                self.stats
                    .inc(StatType::BoundedBacklog, DetailType::LoopScan);

                let batch = guard.index.next(&last, self.config.batch_size);
                // If batch is empty, we iterated over all accounts in the index
                if batch.is_empty() {
                    break;
                }

                drop(guard);
                {
                    let unconfirmed = self.ledger.unconfirmed();
                    for hash in batch {
                        self.stats
                            .inc(StatType::BoundedBacklog, DetailType::Scanned);
                        self.update(&unconfirmed, &hash);
                        last = hash;
                    }
                }
                guard = self.mutex.lock().unwrap();
            }
        }
    }

    fn update(&self, unconfirmed: &impl LedgerSet, hash: &BlockHash) {
        // Erase if the block is either confirmed or missing
        if !unconfirmed.block_exists(hash) {
            self.mutex.lock().unwrap().index.erase_hash(hash);
        }
    }
}

struct BacklogData {
    stopped: bool,
    cool_down: bool,
    index: BacklogIndex,
    ledger: Arc<Ledger>,
    config: BoundedBacklogConfig,
    bucket_count: usize,
    scan_limiter: Mutex<TokenBucket>,
}

impl BacklogData {
    fn predicate(&self) -> bool {
        if self.cool_down {
            return false;
        }

        // Both ledger and tracked backlog must be over the threshold
        self.ledger.backlog_count() > self.config.max_backlog
            && self.index.len() > self.config.max_backlog as usize
    }

    fn gather_targets(
        &self,
        max_count: usize,
        can_rollback: impl Fn(&BlockHash) -> bool,
    ) -> Vec<BlockHash> {
        let mut targets = Vec::new();

        // Start rolling back from lowest index buckets first
        for bucket in 0..self.bucket_count {
            // Only start rolling back if the bucket is over the threshold of unconfirmed blocks
            if self.index.len_of_bucket(bucket) > self.bucket_threshold() {
                let count = min(max_count, self.config.batch_size);
                let top = self.index.top(bucket, count, |hash| {
                    // Only rollback if the block is not being used by the node
                    can_rollback(hash)
                });
                targets.extend(top);
            }
        }
        targets
    }

    fn bucket_threshold(&self) -> usize {
        self.config.max_backlog as usize / self.bucket_count
    }
}

#[derive(Default)]
pub struct BoundedBacklogWriterStats {
    optimistic_successes: AtomicU64,
    optimistic_conflicts: AtomicU64,
    pessimistic_fallbacks: AtomicU64,
    optimistic_active: AtomicU64,
    max_optimistic_concurrency: AtomicU64,
}

impl BoundedBacklogWriterStats {
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
    stats: &'a BoundedBacklogWriterStats,
}

impl Drop for OptimisticWriterGuard<'_> {
    fn drop(&mut self) {
        self.stats.end_optimistic_writer();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_ledger::WriterType;

    #[test]
    fn writer_stats_track_optimistic_rollbacks() {
        let backlog = BoundedBacklog::new_null();
        let stats = backlog.writer_stats();

        backlog
            .backlog_impl
            .roll_back(&[BlockHash::from(1)], 10, |_| true);

        assert!(stats.optimistic_successes() >= 1);
        assert_eq!(stats.optimistic_conflicts(), 0);
        assert_eq!(stats.pessimistic_fallbacks(), 0);
        assert!(stats.max_optimistic_concurrency() >= 1);
    }

    #[test]
    fn bounded_backlog_can_run_alongside_other_writers() {
        let backlog = BoundedBacklog::new_null();
        let stats = backlog.writer_stats();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let barrier_clone = barrier.clone();
        let ledger_clone = backlog.backlog_impl.ledger.clone();

        let handle = std::thread::spawn(move || {
            ledger_clone
                .tx_optimistic_process(WriterType::BlockProcessor, 0, |_txn, _| {
                    barrier_clone.wait();
                    Ok(((), Vec::new()))
                })
                .unwrap();
        });

        backlog
            .backlog_impl
            .roll_back(&[BlockHash::from(2)], 5, |_| true);
        barrier.wait();
        handle.join().unwrap();

        assert!(stats.optimistic_successes() >= 1);
        assert_eq!(stats.optimistic_conflicts(), 0);
        assert_eq!(stats.pessimistic_fallbacks(), 0);
    }
}
