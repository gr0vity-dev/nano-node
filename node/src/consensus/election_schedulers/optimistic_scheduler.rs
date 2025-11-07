use std::{
    cmp::min,
    collections::{HashMap, VecDeque},
    mem::size_of,
    sync::{
        Arc, Condvar, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Instant,
};

use rsnano_ledger::{AnySet, Ledger, LedgerSet};
use rsnano_nullable_clock::SteadyClock;
use rsnano_types::{Account, AccountInfo, ConfirmationHeightInfo};
use rsnano_utils::{
    container_info::ContainerInfo,
    stats::{DetailType, StatType, Stats},
};

use crate::{
    cementation::ConfirmingSet,
    config::NetworkConstants,
    consensus::{ActiveElectionsContainer, AecInsertRequest, election::ElectionBehavior},
};

#[derive(Clone, Debug, PartialEq)]
pub struct OptimisticSchedulerConfig {
    /// Minimum difference between confirmation frontier and account frontier to become a candidate for optimistic confirmation
    pub gap_threshold: u64,

    /// Maximum number of candidates stored in memory
    pub max_size: usize,

    /// Limit of optimistic elections as percentage of `active_elections_size`
    pub optimistic_limit_percentage: usize,
}

impl OptimisticSchedulerConfig {
    pub fn new() -> Self {
        Self {
            gap_threshold: 32,
            max_size: 1024 * 64,
            optimistic_limit_percentage: 10,
        }
    }
}

impl Default for OptimisticSchedulerConfig {
    fn default() -> Self {
        Self::new()
    }
}

pub struct OptimisticScheduler {
    thread: Mutex<Option<JoinHandle<()>>>,
    config: OptimisticSchedulerConfig,
    stopped: AtomicBool,
    condition: Condvar,
    candidates: Mutex<OrderedCandidates>,
    stats: Arc<Stats>,
    active_elections: Arc<RwLock<ActiveElectionsContainer>>,
    network_constants: NetworkConstants,
    ledger: Arc<Ledger>,
    confirming_set: Arc<ConfirmingSet>,
    clock: Arc<SteadyClock>,
    pub max_elections: usize,
}

impl OptimisticScheduler {
    pub fn new(
        config: OptimisticSchedulerConfig,
        stats: Arc<Stats>,
        active_elections: Arc<RwLock<ActiveElectionsContainer>>,
        network_constants: NetworkConstants,
        ledger: Arc<Ledger>,
        confirming_set: Arc<ConfirmingSet>,
        clock: Arc<SteadyClock>,
    ) -> Self {
        let max_elections =
            active_elections.read().unwrap().max_len() * config.optimistic_limit_percentage / 100;

        Self {
            thread: Mutex::new(None),
            config,
            stopped: AtomicBool::new(true),
            condition: Condvar::new(),
            candidates: Mutex::new(OrderedCandidates::default()),
            stats,
            active_elections,
            network_constants,
            ledger,
            confirming_set,
            clock,
            max_elections,
        }
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.notify();
        let handle = self.thread.lock().unwrap().take();
        if let Some(handle) = handle {
            handle.join().unwrap();
        }
    }

    /// Notify about changes in AEC vacancy
    pub fn notify(&self) {
        self.condition.notify_all();
    }

    fn activate_predicate(
        &self,
        account_info: &AccountInfo,
        conf_info: &ConfirmationHeightInfo,
    ) -> bool {
        let big_enough_gap =
            account_info.block_count - conf_info.height > self.config.gap_threshold;

        let nothing_confirmed_yet = conf_info.height == 0;

        big_enough_gap | nothing_confirmed_yet
    }

    /// Called from backlog population to process accounts with unconfirmed blocks
    pub fn activate(
        &self,
        account: &Account,
        account_info: &AccountInfo,
        conf_info: &ConfirmationHeightInfo,
    ) -> bool {
        if self.stopped.load(Ordering::Relaxed) {
            return false;
        }

        if self.activate_predicate(account_info, conf_info) {
            {
                let mut candidates = self.candidates.lock().unwrap();
                // Prevent duplicate candidate accounts
                if candidates.contains(&account) {
                    return false; // Not activated
                }
                // Limit candidates container size
                if candidates.len() >= self.config.max_size {
                    return false; // Not activated
                }

                self.stats
                    .inc(StatType::OptimisticScheduler, DetailType::Activated);
                candidates.insert(*account, Instant::now());
            }
            true // Activated
        } else {
            false // Not activated
        }
    }

    pub fn container_info(&self) -> ContainerInfo {
        let guard = self.candidates.lock().unwrap();
        [(
            "candidates",
            guard.len(),
            size_of::<Account>() * 2 + size_of::<Instant>(),
        )]
        .into()
    }

    fn predicate(&self, candidates: &OrderedCandidates) -> bool {
        let active = self.active_elections.read().unwrap();
        let vacancy = self.max_elections as i64
            - active.count_by_behavior(ElectionBehavior::Optimistic) as i64;
        let vacancy = min(vacancy, active.vacancy());

        if vacancy <= 0 {
            return false;
        }
        if let Some((_account, time)) = candidates.front() {
            time.elapsed() >= self.network_constants.optimistic_activation_delay
        } else {
            false
        }
    }

    fn run(&self) {
        let mut guard = self.candidates.lock().unwrap();
        while !self.stopped.load(Ordering::SeqCst) {
            self.stats
                .inc(StatType::OptimisticScheduler, DetailType::Loop);

            if self.predicate(&guard) {
                let any = self.ledger.any();

                while self.predicate(&guard) {
                    let (account, time) = guard.pop_front().unwrap();
                    drop(guard);
                    self.run_one(&any, account, time);
                    guard = self.candidates.lock().unwrap();
                }
            }

            guard = self
                .condition
                .wait_timeout_while(
                    guard,
                    self.network_constants.optimistic_activation_delay / 2,
                    |g| !self.stopped.load(Ordering::SeqCst) && !self.predicate(g),
                )
                .unwrap()
                .0;
        }
    }

    fn run_one(&self, any: &impl AnySet, account: Account, _time: Instant) {
        let Some(head) = any.account_head(&account) else {
            return;
        };
        if let Some(block) = any.get_block(&head) {
            let forked = false;

            #[cfg(feature = "ledger_snapshots")]
            {
                forked = any.is_forked(&block.qualified_root());
            }

            // Ensure block is not already confirmed
            let is_confirmed = self.confirming_set.contains(&block.hash())
                || any.confirmed().block_exists(&block.hash());

            if !is_confirmed && !forked {
                // Try to insert it into AEC
                // We check for AEC vacancy inside our predicate
                let now = self.clock.now();
                let priority = any.block_priority(&block);
                let inserted = self
                    .active_elections
                    .write()
                    .unwrap()
                    .insert(AecInsertRequest::new_optimistic(block, priority), now)
                    .is_ok();

                if inserted {
                    self.stats
                        .inc(StatType::OptimisticScheduler, DetailType::Insert);
                } else {
                    self.stats
                        .inc(StatType::OptimisticScheduler, DetailType::InsertFailed);
                }
            }
        }
    }
}

impl Drop for OptimisticScheduler {
    fn drop(&mut self) {
        // Thread must be stopped before destruction
        debug_assert!(self.thread.lock().unwrap().is_none())
    }
}

pub trait OptimisticSchedulerExt {
    fn start(&self);
}

impl OptimisticSchedulerExt for Arc<OptimisticScheduler> {
    fn start(&self) {
        debug_assert!(self.thread.lock().unwrap().is_none());
        self.stopped.store(false, Ordering::SeqCst);
        let self_l = Arc::clone(self);
        *self.thread.lock().unwrap() = Some(
            std::thread::Builder::new()
                .name("Sched Opt".to_string())
                .spawn(Box::new(move || {
                    self_l.run();
                }))
                .unwrap(),
        );
    }
}

#[derive(Default)]
struct OrderedCandidates {
    by_account: HashMap<Account, Instant>,
    sequenced: VecDeque<Account>,
}

impl OrderedCandidates {
    fn insert(&mut self, account: Account, time: Instant) {
        if let Some(_) = self.by_account.insert(account, time) {
            self.sequenced.retain(|i| *i != account);
        }
        self.sequenced.push_back(account);
    }

    fn len(&self) -> usize {
        self.sequenced.len()
    }

    fn contains(&self, account: &Account) -> bool {
        self.by_account.contains_key(account)
    }

    fn front(&self) -> Option<(Account, Instant)> {
        self.sequenced
            .front()
            .and_then(|account| self.by_account.get(account).map(|time| (*account, *time)))
    }

    fn pop_front(&mut self) -> Option<(Account, Instant)> {
        self.sequenced.pop_front().map(|account| {
            let time = self.by_account.remove(&account).unwrap();
            (account, time)
        })
    }
}
