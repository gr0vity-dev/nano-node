mod activation_loop;
mod election_schedulers_plugin;
mod hinted_scheduler;
mod manual_scheduler;
mod optimistic;
pub mod priority;

use activation_loop::ActivationLoop;
use activation_loop::SchedulerRuntimeWakeHandle;
pub(crate) use election_schedulers_plugin::*;
pub use hinted_scheduler::*;
pub use manual_scheduler::*;
pub use optimistic::*;

use std::sync::{Arc, Mutex};

use rsnano_ledger::{AnySet, Ledger, ProcessResult};
use rsnano_nullable_clock::SteadyClock;
use rsnano_types::{Account, AccountInfo, BlockHash, ConfirmationHeightInfo, SavedBlock};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{Stats, StatsCollection, StatsSource},
};

use super::{AecService, VoteCache};
use crate::{cementation::ConfirmingSet, config::NodeConfig, representatives::OnlineReps};
use priority::PriorityScheduler;

#[derive(Clone)]
pub(crate) struct AecSchedulerWaker {
    runtime_wake: SchedulerRuntimeWakeHandle,
}

impl AecSchedulerWaker {
    pub(crate) fn wake_after_vacancy_release_or_recovery(&self) {
        self.runtime_wake.wake();
    }
}

#[derive(Clone)]
pub(crate) struct HintedSchedulerWaker {
    runtime_wake: SchedulerRuntimeWakeHandle,
}

impl HintedSchedulerWaker {
    pub(crate) fn wake_after_vote_cache_change(&self) {
        self.runtime_wake.wake();
    }
}

pub(crate) struct ElectionSchedulerWakers {
    runtime_wake: SchedulerRuntimeWakeHandle,
}

impl ElectionSchedulerWakers {
    pub(crate) fn new() -> Self {
        Self {
            runtime_wake: SchedulerRuntimeWakeHandle::new(),
        }
    }

    pub(crate) fn aec(&self) -> AecSchedulerWaker {
        AecSchedulerWaker {
            runtime_wake: self.runtime_wake.clone(),
        }
    }

    pub(crate) fn hinted(&self) -> HintedSchedulerWaker {
        HintedSchedulerWaker {
            runtime_wake: self.runtime_wake.clone(),
        }
    }

    fn runtime(&self) -> SchedulerRuntimeWakeHandle {
        self.runtime_wake.clone()
    }
}

pub struct ElectionSchedulers {
    pub(crate) priority: Arc<PriorityScheduler>,
    pub(crate) optimistic: Arc<OptimisticScheduler>,
    pub(crate) hinted: Arc<HintedScheduler>,
    pub(crate) manual: Arc<ManualScheduler>,
    activation_loop: Arc<ActivationLoop>,
    wake_handle: SchedulerRuntimeWakeHandle,
    ledger: Arc<Ledger>,
}

impl ElectionSchedulers {
    pub(crate) fn new(
        config: NodeConfig,
        active_elections: Arc<AecService>,
        ledger: Arc<Ledger>,
        stats: Arc<Stats>,
        vote_cache: Arc<Mutex<VoteCache>>,
        confirming_set: Arc<ConfirmingSet>,
        online_reps: Arc<Mutex<OnlineReps>>,
        clock: Arc<SteadyClock>,
        wakers: &ElectionSchedulerWakers,
    ) -> Self {
        let hinted = Arc::new(HintedScheduler::new(
            config.hinted_scheduler.clone(),
            active_elections.clone(),
            ledger.clone(),
            stats.clone(),
            vote_cache.clone(),
            confirming_set.clone(),
            online_reps.clone(),
            clock.clone(),
        ));

        let manual = Arc::new(ManualScheduler::new(
            stats.clone(),
            active_elections.clone(),
            clock.clone(),
            ledger.clone(),
        ));

        let optimistic_params = OptimisticSchedulerParams {
            gap_threshold: config.optimistic_scheduler.gap_threshold,
            max_candidates: config.optimistic_scheduler.max_size,
            max_elections: config.active_elections.max_elections
                * config.optimistic_scheduler.optimistic_limit_percentage
                / 100,
            activation_delay: config.optimistic_scheduler.activation_delay,
        };
        let optimistic = Arc::new(OptimisticScheduler::new(
            optimistic_params,
            active_elections.clone(),
            ledger.clone(),
            confirming_set.clone(),
            clock.clone(),
        ));

        let priority = Arc::new(PriorityScheduler::new(
            config.priority_bucket.clone(),
            stats.clone(),
            active_elections.clone(),
            clock,
        ));

        let activation_loop = Arc::new(ActivationLoop::new(
            wakers.runtime(),
            priority.clone(),
            optimistic.clone(),
            hinted.clone(),
            manual.clone(),
            config.enable_priority_scheduler,
            config.enable_optimistic_scheduler,
            config.enable_hinted_scheduler,
        ));

        Self {
            priority,
            optimistic,
            hinted,
            manual,
            activation_loop,
            wake_handle: wakers.runtime(),
            ledger,
        }
    }

    pub fn new_null() -> Self {
        let config = NodeConfig::new_test_instance();
        let active_elections = Arc::new(AecService::new_null());
        let ledger = Arc::new(Ledger::new_null());
        let stats = Arc::new(Stats::default());
        let vote_cache = Arc::new(Mutex::new(VoteCache::new(
            Default::default(),
            stats.clone(),
        )));
        let confirming_set = Arc::new(ConfirmingSet::new_null());
        let online_reps = Arc::new(Mutex::new(OnlineReps::new_test_instance()));
        let clock = Arc::new(SteadyClock::new_null());
        let wakers = ElectionSchedulerWakers::new();

        Self::new(
            config,
            active_elections,
            ledger,
            stats,
            vote_cache,
            confirming_set,
            online_reps,
            clock,
            &wakers,
        )
    }

    /// Does the block exist in any of the schedulers
    pub fn contains(&self, hash: &BlockHash) -> bool {
        self.manual.contains(hash) || self.priority.contains(hash)
    }

    pub fn activate_backlog(
        &self,
        any: &impl AnySet,
        account: &Account,
        account_info: &AccountInfo,
        conf_info: &ConfirmationHeightInfo,
    ) {
        self.optimistic
            .activate(account, account_info.block_count, conf_info.height);
        self.priority.activate_backlog(any, account_info, conf_info);
        self.wake_handle.wake();
    }

    pub fn activate_accounts_with_fresh_blocks(&self, processed: &[ProcessResult]) {
        let any = self.ledger.any();
        self.priority
            .activate_accounts_with_fresh_blocks(&any, processed);
        self.wake_handle.wake();
    }

    pub fn add_manual(&self, block: SavedBlock) {
        self.manual.push(block);
        self.wake_handle.wake();
    }

    pub fn activate_successors<'a>(&self, confirmed: impl IntoIterator<Item = &'a SavedBlock>) {
        let any = self.ledger.any();
        self.priority.activate_successors(&any, confirmed);
        self.wake_handle.wake();
    }

    pub fn start(&self) {
        self.activation_loop.start();
    }

    pub fn stop(&self) {
        self.activation_loop.stop();
    }
}

impl ContainerInfoProvider for ElectionSchedulers {
    fn container_info(&self) -> ContainerInfo {
        ContainerInfo::builder()
            .node("hinted", self.hinted.container_info())
            .node("manual", self.manual.container_info())
            .node("optimistic", self.optimistic.container_info())
            .node("priority", self.priority.container_info())
            .finish()
    }
}

impl StatsSource for ElectionSchedulers {
    fn collect_stats(&self, result: &mut StatsCollection) {
        self.priority.collect_stats(result);
        self.optimistic.collect_stats(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activate_successors() {
        let schedulers = ElectionSchedulers::new_null();
        let tracker = schedulers.priority.track_activate_successors();
        let block = SavedBlock::new_test_instance();

        schedulers.activate_successors([&block]);

        let output = tracker.output();
        assert_eq!(output, [block]);
    }
}
