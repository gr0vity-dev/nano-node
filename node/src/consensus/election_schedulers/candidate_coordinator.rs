use std::{
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
};

use rsnano_ledger::{AnySet, ConfirmedSet};
use rsnano_nullable_clock::SteadyClock;
use rsnano_output_tracker::OutputListenerMt;
#[cfg(test)]
use rsnano_output_tracker::OutputTrackerMt;
use rsnano_types::{Account, AccountInfo, BlockHash, ConfirmationHeightInfo, SavedBlock};
use rsnano_utils::{
    container_info::ContainerInfo,
    stats::{DetailType, StatType, Stats, StatsCollection, StatsSource},
};

use super::priority::{
    BucketInsertError, Eviction, PriorityBucketConfig, PriorityBuckets, prio_bucket_count,
};
use crate::consensus::AecService;

pub(crate) struct CandidateCoordinator {
    stopped: Mutex<bool>,
    condition: Condvar,
    stats: Arc<Stats>,
    priority_buckets: Mutex<PriorityBuckets>,
    priority_thread: Mutex<Option<JoinHandle<()>>>,
    clock: Arc<SteadyClock>,
    aec: Arc<AecService>,
    activate_successors_listener: OutputListenerMt<SavedBlock>,
}

impl CandidateCoordinator {
    pub(crate) fn new(
        priority_config: PriorityBucketConfig,
        stats: Arc<Stats>,
        active_elections: Arc<AecService>,
        clock: Arc<SteadyClock>,
    ) -> Self {
        let priority_buckets = PriorityBuckets::new(prio_bucket_count(), priority_config);
        Self {
            stopped: Mutex::new(false),
            condition: Condvar::new(),
            stats,
            priority_buckets: Mutex::new(priority_buckets),
            priority_thread: Mutex::new(None),
            clock,
            aec: active_elections,
            activate_successors_listener: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn track_activate_successors(&self) -> Arc<OutputTrackerMt<SavedBlock>> {
        self.activate_successors_listener.track()
    }

    pub(crate) fn start_loop(self: &Arc<Self>) {
        debug_assert!(self.priority_thread.lock().unwrap().is_none());

        let self_l = Arc::clone(self);
        *self.priority_thread.lock().unwrap() = Some(
            std::thread::Builder::new()
                .name("Sched Priority".to_string())
                .spawn(Box::new(move || {
                    self_l.run();
                }))
                .unwrap(),
        );
    }

    pub(crate) fn stop(&self) {
        *self.stopped.lock().unwrap() = true;
        self.condition.notify_all();
        if let Some(handle) = self.priority_thread.lock().unwrap().take() {
            handle.join().unwrap();
        }
    }

    pub(crate) fn notify(&self) {
        self.condition.notify_all();
    }

    pub(crate) fn contains(&self, hash: &BlockHash) -> bool {
        self.priority_buckets.lock().unwrap().contains(hash)
    }

    pub(crate) fn activate_priority(&self, any: &impl AnySet, account: &Account) {
        debug_assert!(!account.is_zero());
        if let Some(account_info) = any.get_account(account) {
            let conf_info = any.confirmed().get_conf_info(account).unwrap_or_default();

            if conf_info.height < account_info.block_count {
                self.activate_priority_with_info(any, &account_info, &conf_info);
                return;
            }
        };

        self.stats
            .inc(StatType::ElectionScheduler, DetailType::ActivateSkip);
    }

    pub(crate) fn activate_priority_with_info(
        &self,
        any: &impl AnySet,
        account_info: &AccountInfo,
        conf_info: &ConfirmationHeightInfo,
    ) {
        debug_assert!(conf_info.frontier != account_info.head);

        let next_unconfirmed_hash = match conf_info.height {
            0 => account_info.open_block,
            _ => match any.block_successor(&conf_info.frontier) {
                Some(h) => h,
                None => return,
            },
        };

        let Some(block) = any.get_block(&next_unconfirmed_hash) else {
            return;
        };

        if !any.dependencies_confirmed(&block) {
            self.stats
                .inc(StatType::ElectionScheduler, DetailType::ActivateFailed);
            return;
        }

        #[cfg(feature = "ledger_snapshots")]
        if any.is_forked(&block.qualified_root()) {
            self.stats
                .inc(StatType::ElectionScheduler, DetailType::ActivateFailed);
            return;
        }

        let priority = any.block_priority(&block);
        let insert_result = self
            .priority_buckets
            .lock()
            .unwrap()
            .insert(priority, block);

        match insert_result {
            Ok(Eviction::None) => {}
            Ok(Eviction::Evicted) => {
                self.stats
                    .inc(StatType::ElectionScheduler, DetailType::Evicted);
            }
            Err(BucketInsertError::Duplicate) => {
                self.stats
                    .inc(StatType::ElectionScheduler, DetailType::Duplicate);
            }
            Err(BucketInsertError::PriorityTooLow) => {
                self.stats
                    .inc(StatType::ElectionScheduler, DetailType::ActivateFull);
            }
        }

        if insert_result.is_ok() {
            self.stats
                .inc(StatType::ElectionScheduler, DetailType::Activated);
            self.condition.notify_all();
        }
    }

    pub(crate) fn activate_successors(&self, any: &impl AnySet, block: &SavedBlock) {
        if self.activate_successors_listener.is_tracked() {
            self.activate_successors_listener.emit(block.clone());
        }
        self.activate_priority(any, &block.account());
        self.activate_destination_account(any, block);
    }

    pub(crate) fn container_info(&self) -> ContainerInfo {
        let mut bucket_infos = ContainerInfo::builder();
        for (id, bucket) in self.priority_buckets.lock().unwrap().iter().enumerate() {
            bucket_infos = bucket_infos.leaf(id.to_string(), bucket.len(), 0);
        }

        ContainerInfo::builder()
            .node("blocks", bucket_infos.finish())
            .finish()
    }

    fn activate_destination_account(&self, any: &impl AnySet, block: &SavedBlock) {
        if let Some(destination) = block.destination()
            && block.is_send()
            && !destination.is_zero()
            && destination != block.account()
        {
            self.activate_priority(any, &destination);
        }
    }

    fn run(&self) {
        let mut stopped = self.stopped.lock().unwrap();
        while !*stopped {
            stopped = self
                .condition
                .wait_while(stopped, |s| !*s && !self.priority_predicate())
                .unwrap();

            if !*stopped {
                drop(stopped);
                self.run_priority_refill();
                stopped = self.stopped.lock().unwrap();
            }
        }
    }

    fn priority_predicate(&self) -> bool {
        let buckets = self.priority_buckets.lock().unwrap();
        self.aec.check_vacancy(&*buckets)
    }

    fn run_priority_refill(&self) {
        self.stats
            .inc(StatType::ElectionScheduler, DetailType::Loop);

        let now = self.clock.now();
        let mut buckets = self.priority_buckets.lock().unwrap();
        self.aec.refill(&mut *buckets, now);
    }
}

impl Drop for CandidateCoordinator {
    fn drop(&mut self) {
        debug_assert!(self.priority_thread.lock().unwrap().is_none());
    }
}

impl StatsSource for CandidateCoordinator {
    fn collect_stats(&self, result: &mut StatsCollection) {
        let guard = self.priority_buckets.lock().unwrap();
        guard.bucket_stats.collect_stats(result);
        guard.collect_stats(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_ledger::{Ledger, LedgerInserter};
    use rsnano_types::PrivateKey;

    #[test]
    fn can_track_successor_activation() {
        let coordinator = create_test_coordinator();
        let block = SavedBlock::new_test_instance();
        let ledger = Ledger::new_null();
        let tracker = coordinator.track_activate_successors();

        coordinator.activate_successors(&ledger.any(), &block);

        let output = tracker.output();
        assert_eq!(output, [block]);
    }

    #[test]
    fn activate_successors() {
        let coordinator = create_test_coordinator();

        let ledger = Ledger::new_null();
        let inserter = LedgerInserter::new(&ledger);
        let destination = PrivateKey::from(1);
        let send1 = inserter.genesis().send(&destination, 100);
        let send2 = inserter.genesis().send(Account::from(2), 100);
        let open = inserter.account(&destination).receive(send1.hash());

        ledger.confirm(send1.hash());
        coordinator.activate_successors(&ledger.any(), &send1);
        coordinator.run_priority_refill();

        assert!(coordinator.aec.is_active_hash(&send2.hash()));
        assert!(coordinator.aec.is_active_hash(&open.hash()));
    }

    fn create_test_coordinator() -> CandidateCoordinator {
        let config = PriorityBucketConfig::default();
        let stats = Arc::new(Stats::default());
        let active_elections = Arc::new(AecService::new_null());
        let clock = Arc::new(SteadyClock::new_null());
        CandidateCoordinator::new(config, stats, active_elections, clock)
    }
}
