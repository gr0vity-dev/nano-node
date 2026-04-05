use std::sync::{Arc, Mutex};

use rsnano_ledger::{AnySet, ConfirmedSet, ProcessResult};
use rsnano_nullable_clock::SteadyClock;
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::{Account, AccountInfo, BlockHash, ConfirmationHeightInfo, SavedBlock};
use rsnano_utils::{
    container_info::ContainerInfo,
    stats::{DetailType, StatType, Stats, StatsCollection, StatsSource},
};

use super::{PriorityBucketConfig, prio_bucket_count};
use crate::consensus::{
    AecService,
    election_schedulers::priority::{
        BucketInsertError, Eviction, priority_buckets::PriorityBuckets,
    },
};

pub struct PriorityScheduler {
    stats: Arc<Stats>,
    buckets: Mutex<PriorityBuckets>,
    clock: Arc<SteadyClock>,
    aec: Arc<AecService>,
    activate_successors_listener: OutputListenerMt<SavedBlock>,
}

impl PriorityScheduler {
    pub(crate) fn new(
        config: PriorityBucketConfig,
        stats: Arc<Stats>,
        active_elections: Arc<AecService>,
        clock: Arc<SteadyClock>,
    ) -> Self {
        let buckets = PriorityBuckets::new(prio_bucket_count(), config);

        Self {
            buckets: Mutex::new(buckets),
            stats,
            clock,
            aec: active_elections,
            activate_successors_listener: Default::default(),
        }
    }

    pub fn track_activate_successors(&self) -> Arc<OutputTrackerMt<SavedBlock>> {
        self.activate_successors_listener.track()
    }

    pub fn contains(&self, hash: &BlockHash) -> bool {
        self.buckets.lock().unwrap().contains(hash)
    }

    pub fn activate(&self, any: &impl AnySet, account: &Account) {
        debug_assert!(!account.is_zero());
        if let Some(account_info) = any.get_account(account) {
            let conf_info = any.confirmed().get_conf_info(account).unwrap_or_default();

            if conf_info.height < account_info.block_count {
                self.activate_with_info(any, &account_info, &conf_info);
                return;
            }
        };

        self.stats
            .inc(StatType::ElectionScheduler, DetailType::ActivateSkip);
    }

    pub fn activate_with_info(
        &self,
        any: &impl AnySet,
        account_info: &AccountInfo,
        conf_info: &ConfirmationHeightInfo,
    ) {
        debug_assert!(conf_info.frontier != account_info.head);

        let next_unconfirmed_hash = match conf_info.height {
            0 => account_info.open_block,
            _ => {
                match any.block_successor(&conf_info.frontier) {
                    Some(h) => h,
                    None => {
                        // This can happen if the bounded backlog did a rollback
                        return;
                    }
                }
            }
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

        let insert_result = self.buckets.lock().unwrap().insert(priority, block);

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
        }
    }

    pub fn activate_backlog(
        &self,
        any: &impl AnySet,
        account_info: &AccountInfo,
        conf_info: &ConfirmationHeightInfo,
    ) {
        self.activate_with_info(any, account_info, conf_info);
    }

    pub fn activate_accounts_with_fresh_blocks(
        &self,
        any: &impl AnySet,
        processed: &[ProcessResult],
    ) {
        for result in processed {
            if result.status.is_ok()
                && let Some(saved_block) = result.saved_block.as_ref()
            {
                self.activate(any, &saved_block.account());
            }
        }
    }

    pub fn len(&self) -> usize {
        self.buckets.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn predicate(&self) -> bool {
        let buckets = self.buckets.lock().unwrap();
        self.aec.check_vacancy(&*buckets)
    }

    pub fn run_one(&self) -> bool {
        if !self.predicate() {
            return false;
        }

        self.stats
            .inc(StatType::ElectionScheduler, DetailType::Loop);

        let now = self.clock.now();
        let mut buckets = self.buckets.lock().unwrap();
        self.aec.refill(&mut *buckets, now);
        true
    }

    pub fn activate_successors<'a>(
        &self,
        any: &impl AnySet,
        confirmed: impl IntoIterator<Item = &'a SavedBlock>,
    ) {
        for block in confirmed {
            if self.activate_successors_listener.is_tracked() {
                self.activate_successors_listener.emit(block.clone());
            }
            self.activate(any, &block.account());
            self.activate_destination_account(any, block);
        }
    }

    fn activate_destination_account(&self, any: &impl AnySet, block: &SavedBlock) {
        if let Some(destination) = block.destination()
            && block.is_send()
            && !destination.is_zero()
            && destination != block.account()
        {
            self.activate(any, &destination);
        }
    }

    pub fn container_info(&self) -> ContainerInfo {
        let mut bucket_infos = ContainerInfo::builder();

        for (id, bucket) in self.buckets.lock().unwrap().iter().enumerate() {
            bucket_infos = bucket_infos.leaf(id.to_string(), bucket.len(), 0);
        }

        ContainerInfo::builder()
            .node("blocks", bucket_infos.finish())
            .finish()
    }
}

impl StatsSource for PriorityScheduler {
    fn collect_stats(&self, result: &mut StatsCollection) {
        let guard = self.buckets.lock().unwrap();
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
        let scheduler = create_test_scheduler();
        let block = SavedBlock::new_test_instance();
        let ledger = Ledger::new_null();
        let tracker = scheduler.track_activate_successors();

        scheduler.activate_successors(&ledger.any(), [&block]);

        let output = tracker.output();
        assert_eq!(output, [block]);
    }

    #[test]
    fn activate_successors() {
        let scheduler = create_test_scheduler();

        let ledger = Ledger::new_null();
        let inserter = LedgerInserter::new(&ledger);
        let destination = PrivateKey::from(1);
        let send1 = inserter.genesis().send(&destination, 100);
        let send2 = inserter.genesis().send(Account::from(2), 100);
        let open = inserter.account(&destination).receive(send1.hash());

        ledger.confirm(send1.hash());
        scheduler.activate_successors(&ledger.any(), [&send1]);
        scheduler.run_one();

        assert!(scheduler.aec.is_active_hash(&send2.hash()));
        assert!(scheduler.aec.is_active_hash(&open.hash()));
    }

    fn create_test_scheduler() -> PriorityScheduler {
        let config = PriorityBucketConfig::default();
        let stats = Arc::new(Stats::default());
        let active_elections = Arc::new(AecService::new_null());
        let clock = Arc::new(SteadyClock::new_null());
        PriorityScheduler::new(config, stats, active_elections, clock)
    }
}
