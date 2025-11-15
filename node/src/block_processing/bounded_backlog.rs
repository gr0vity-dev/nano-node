use std::{
    cmp::min,
    sync::{Arc, Condvar, Mutex, RwLock},
    thread::JoinHandle,
    time::Duration,
};

use tracing::warn;

use rsnano_ledger::{AnySet, Ledger, LedgerSet, OwningAnySet};
use rsnano_network::token_bucket::TokenBucket;
use rsnano_nullable_clock::SteadyClock;
use rsnano_types::{Account, AccountInfo, BlockHash, ConfirmationHeightInfo, SavedBlock};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{DetailType, StatType, Stats},
    sync::backpressure_channel::{Sender, channel},
};

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
        });

        Self {
            backlog_impl,
            thread: Mutex::new(None),
            scan_thread: Mutex::new(None),
        }
    }

    pub fn new_null() -> Self {
        let config = BoundedBacklogConfig::default();
        let ledger = Arc::new(Ledger::new_null(
            rsnano_store_lmdb::null_ledger_store_factory(),
        ));
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
        let results = self
            .ledger
            .roll_back_batch(targets, max_rollbacks, can_roll_back);

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
