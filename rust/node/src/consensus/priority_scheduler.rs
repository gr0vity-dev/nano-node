use super::{ActiveTransactions, Buckets, ElectionBehavior};
use crate::{
    consensus::ActiveTransactionsExt,
    stats::{DetailType, StatType, Stats},
};
use rsnano_core::{utils::ContainerInfoComponent, Account, BlockEnum};
use rsnano_ledger::Ledger;
use rsnano_store_lmdb::{LmdbReadTransaction, RoCursorWrapper, Transaction};
use std::{
    cmp::max,
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
};
use tracing::trace;

pub struct PriorityScheduler {
    thread: Mutex<Option<JoinHandle<()>>>,
    mutex: Mutex<PrioritySchedulerImpl>,
    condition: Condvar,
    ledger: Arc<Ledger>,
    stats: Arc<Stats>,
    active: Arc<ActiveTransactions>,
}

impl PriorityScheduler {
    pub fn new(ledger: Arc<Ledger>, stats: Arc<Stats>, active: Arc<ActiveTransactions>) -> Self {
        Self {
            thread: Mutex::new(None),
            mutex: Mutex::new(PrioritySchedulerImpl {
                stopped: false,
                buckets: Buckets::default(),
            }),
            condition: Condvar::new(),
            ledger,
            stats,
            active,
        }
    }

    pub fn stop(&self) {
        self.mutex.lock().unwrap().stopped = true;
        self.notify();
        if let Some(handle) = self.thread.lock().unwrap().take() {
            handle.join().unwrap();
        }
    }

    pub fn activate(
        &self,
        account: &Account,
        tx: &dyn Transaction<Database = lmdb::Database, RoCursor = RoCursorWrapper>,
    ) -> bool {
        debug_assert!(!account.is_zero());
        let Some(info) = self.ledger.account_info(tx, account) else {
            return false; // Not activated
        };

        let conf_info = self
            .ledger
            .store
            .confirmation_height
            .get(tx, account)
            .unwrap_or_default();

        if conf_info.height >= info.block_count {
            return false; // Not activated
        }

        debug_assert!(conf_info.frontier != info.head);
        let hash = if conf_info.height == 0 {
            info.open_block
        } else {
            self.ledger
                .successor(tx, &conf_info.frontier)
                .unwrap_or_default()
        };
        let block = self.ledger.get_block(tx, &hash).unwrap();
        if !self.ledger.dependents_confirmed(tx, &block) {
            return false;
        }

        let balance = self.ledger.balance(tx, &hash).unwrap();
        let previous_balance = self
            .ledger
            .balance(tx, &conf_info.frontier)
            .unwrap_or_default();
        let balance_priority = max(balance, previous_balance);

        self.stats
            .inc(StatType::ElectionScheduler, DetailType::Activated);

        trace!(
            account = account.encode_account(),
            ?block,
            time = info.modified,
            priority = balance_priority.number(),
            "priority scheduler activated"
        );

        let mut guard = self.mutex.lock().unwrap();
        guard
            .buckets
            .push(info.modified, Arc::new(block), balance_priority);
        self.notify();

        true // Activated
    }

    pub fn notify(&self) {
        self.condition.notify_all();
    }

    pub fn len(&self) -> usize {
        self.mutex.lock().unwrap().buckets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mutex.lock().unwrap().buckets.is_empty()
    }

    fn predicate(&self, buckets: &Buckets) -> bool {
        self.active.vacancy(ElectionBehavior::Normal) > 0 && !buckets.is_empty()
    }

    pub fn run(&self) {
        let mut guard = self.mutex.lock().unwrap();
        while !guard.stopped {
            guard = self
                .condition
                .wait_while(guard, |i| !i.stopped && !self.predicate(&i.buckets))
                .unwrap();
            if !guard.stopped {
                self.stats
                    .inc(StatType::ElectionScheduler, DetailType::Loop);

                if self.predicate(&guard.buckets) {
                    let block = Arc::clone(guard.buckets.top());
                    guard.buckets.pop();
                    drop(guard);
                    self.stats
                        .inc(StatType::ElectionScheduler, DetailType::InsertPriority);
                    let (inserted, election) = self.active.insert(&block, ElectionBehavior::Normal);
                    if inserted {
                        self.stats.inc(
                            StatType::ElectionScheduler,
                            DetailType::InsertPrioritySuccess,
                        );
                    }
                    if let Some(election) = election {
                        election.transition_active();
                    }
                } else {
                    drop(guard);
                }
                self.notify();
                guard = self.mutex.lock().unwrap();
            }
        }
    }

    pub fn activate_successors(&self, tx: &LmdbReadTransaction, block: &BlockEnum) {
        self.activate(&block.account(), tx);

        // Start or vote for the next unconfirmed block in the destination account
        if let Some(destination) = block.destination() {
            if block.is_send() && !destination.is_zero() && destination != block.account() {
                self.activate(&destination, tx);
            }
        }
    }

    pub fn collect_container_info(&self, name: impl Into<String>) -> ContainerInfoComponent {
        let guard = self.mutex.lock().unwrap();
        ContainerInfoComponent::Composite(
            name.into(),
            vec![guard.buckets.collect_container_info("buckets")],
        )
    }
}

impl Drop for PriorityScheduler {
    fn drop(&mut self) {
        // Thread must be stopped before destruction
        debug_assert!(self.thread.lock().unwrap().is_none());
    }
}

pub trait PrioritySchedulerExt {
    fn start(&self);
}

impl PrioritySchedulerExt for Arc<PriorityScheduler> {
    fn start(&self) {
        debug_assert!(self.thread.lock().unwrap().is_none());
        let self_l = Arc::clone(&self);
        *self.thread.lock().unwrap() = Some(
            std::thread::Builder::new()
                .name("Sched Priority".to_string())
                .spawn(Box::new(move || {
                    self_l.run();
                }))
                .unwrap(),
        );
    }
}

struct PrioritySchedulerImpl {
    stopped: bool,
    buckets: Buckets,
}
