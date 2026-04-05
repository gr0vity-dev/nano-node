use std::{
    sync::{Arc, Mutex, atomic::Ordering::Relaxed},
    time::Duration,
};

use rsnano_ledger::{AnySet, Ledger, LedgerSet, OwningAnySet};
use rsnano_nullable_clock::SteadyClock;
use rsnano_types::Account;
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{StatsCollection, StatsSource},
};

use crate::{
    cementation::ConfirmingSet,
    consensus::{AecInsertRequest, AecService, election::ElectionBehavior},
};

mod candidate_queue;
mod config;
mod logic;
mod stats;

use candidate_queue::CandidateQueue;
pub use config::OptimisticSchedulerParams;
use logic::OptimisticSchedulerLogic;
use stats::OptimisticSchedulerStats;

pub struct OptimisticScheduler {
    state: Mutex<OptimisticSchedulerState>,
    logic: OptimisticSchedulerLogic,
    aec: Arc<AecService>,
    ledger: Arc<Ledger>,
    confirming_set: Arc<ConfirmingSet>,
    clock: Arc<SteadyClock>,
    max_elections: usize,
    stats: OptimisticSchedulerStats,
}

impl OptimisticScheduler {
    pub fn new(
        params: OptimisticSchedulerParams,
        aec: Arc<AecService>,
        ledger: Arc<Ledger>,
        confirming_set: Arc<ConfirmingSet>,
        clock: Arc<SteadyClock>,
    ) -> Self {
        Self {
            max_elections: params.max_elections,
            logic: OptimisticSchedulerLogic::new(params),
            state: Mutex::new(OptimisticSchedulerState::default()),
            aec,
            ledger,
            confirming_set,
            clock,
            stats: OptimisticSchedulerStats::default(),
        }
    }

    pub fn max_elections(&self) -> usize {
        self.max_elections
    }

    pub fn activation_delay(&self) -> Duration {
        self.logic.activation_delay()
    }

    pub fn stop(&self) {
        self.state.lock().unwrap().stopped = true;
    }

    /// Called from backlog population to process accounts with unconfirmed blocks
    pub fn activate(&self, account: &Account, block_count: u64, confirmation_height: u64) -> bool {
        let now = self.clock.now();
        let mut state = self.state.lock().unwrap();
        if state.stopped {
            return false;
        }
        let activated = self.logic.try_activate(
            &mut state.candidates,
            account,
            block_count,
            confirmation_height,
            now,
        );
        if activated {
            self.stats.activated_count.fetch_add(1, Relaxed);
        }
        activated
    }

    pub fn run_one(&self) -> bool {
        self.stats.loop_count.fetch_add(1, Relaxed);

        let account = {
            let mut state = self.state.lock().unwrap();
            if state.stopped || !self.has_vacancy() {
                return false;
            }

            let now = self.clock.now();
            let Some(account) = self.logic.pop_candidate(&mut state.candidates, now) else {
                return false;
            };
            account
        };

        let any = self.ledger.any();
        self.run_account(&any, account);
        true
    }

    fn run_account(&self, any: &OwningAnySet, account: Account) {
        let Some(head) = any.account_head(&account) else {
            return;
        };
        let Some(block) = any.get_block(&head) else {
            return;
        };

        #[cfg(feature = "ledger_snapshots")]
        {
            if any.is_forked(&block.qualified_root()) {
                // Needed for new consensus algorithm in ledger snapshot.
                // We never vote for forked blocks.
                return;
            }
        }

        let is_confirmed = self.confirming_set.contains(&block.hash())
            || any.confirmed().block_exists(&block.hash());

        if is_confirmed {
            return;
        }

        let now = self.clock.now();
        let priority = any.block_priority(&block);
        let inserted = self
            .aec
            .insert(AecInsertRequest::new_optimistic(block, priority), now)
            .is_ok();

        if inserted {
            self.stats.insert_count.fetch_add(1, Relaxed);
        } else {
            self.stats.insert_failed_count.fetch_add(1, Relaxed);
        }
    }

    fn has_vacancy(&self) -> bool {
        let optimistic_count = self.aec.count_by_behavior(ElectionBehavior::Optimistic);
        let aec_vacancy = self.aec.vacancy();
        self.logic.has_vacancy(optimistic_count, aec_vacancy)
    }

    #[cfg(test)]
    pub fn candidate_count(&self) -> usize {
        self.state.lock().unwrap().candidates.len()
    }
}

impl StatsSource for OptimisticScheduler {
    fn collect_stats(&self, result: &mut StatsCollection) {
        self.stats.collect_stats(result);
    }
}

impl ContainerInfoProvider for OptimisticScheduler {
    fn container_info(&self) -> ContainerInfo {
        [("candidates", self.state.lock().unwrap().candidates.len(), 1)].into()
    }
}

#[derive(Default)]
struct OptimisticSchedulerState {
    stopped: bool,
    candidates: CandidateQueue,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_ledger::{ConfirmedSet, test_helpers::UnsavedBlockLatticeBuilder};
    use rsnano_types::PrivateKey;

    #[test]
    fn stop_sets_stopped_flag() {
        let scheduler = make_scheduler();

        scheduler.stop();

        assert!(!scheduler.activate(&Account::from(1), TEST_GAP_THRESHOLD + 1, 0));
        assert_eq!(scheduler.candidate_count(), 0);
    }

    #[test]
    fn schedules_election_when_over_gap_threshold() {
        let aec = Arc::new(AecService::new_null());
        let ledger = Arc::new(Ledger::new_null());
        let scheduler = make_scheduler_with(aec.clone(), ledger.clone());

        let mut builder = UnsavedBlockLatticeBuilder::with_stub_work();
        for _ in 0..TEST_GAP_THRESHOLD {
            let block = builder.genesis().send(1, 1);
            ledger.process_one(&block).unwrap();
        }

        assert!(activate(&scheduler, ledger.genesis().account()));

        while scheduler.run_one() {}

        let optimistic_count = aec.count_by_behavior(ElectionBehavior::Optimistic);

        assert_eq!(optimistic_count, 1, "should schedule the election");
        assert_eq!(
            scheduler.candidate_count(),
            0,
            "should remove the candidate"
        );
    }

    #[test]
    fn schedules_election_when_account_is_unconfirmed() {
        let aec = Arc::new(AecService::new_null());
        let ledger = Arc::new(Ledger::new_null());
        let scheduler = make_scheduler_with(aec.clone(), ledger.clone());

        let mut builder = UnsavedBlockLatticeBuilder::with_stub_work();
        let unconf_account = PrivateKey::from(42);

        let count = TEST_GAP_THRESHOLD as usize + 1;
        let sends: Vec<_> = (0..count)
            .map(|_| builder.genesis().send(&unconf_account, 1))
            .collect();
        let receives: Vec<_> = sends
            .iter()
            .map(|s| builder.account(&unconf_account).receive(s))
            .collect();
        for s in &sends {
            ledger.process_one(s).unwrap();
        }
        for r in &receives {
            ledger.process_one(r).unwrap();
        }
        let head = receives.last().unwrap();

        assert!(activate(&scheduler, unconf_account.account()));

        while scheduler.run_one() {}

        let optimistic_count = aec.count_by_behavior(ElectionBehavior::Optimistic);

        assert_eq!(optimistic_count, 1, "should schedule the election");
        assert!(aec.is_active_hash(&head.hash()));
    }

    #[test]
    fn does_not_schedule_when_gap_is_under_threshold() {
        let aec = Arc::new(AecService::new_null());
        let ledger = Arc::new(Ledger::new_null());
        let scheduler = make_scheduler_with(aec.clone(), ledger.clone());

        let mut builder = UnsavedBlockLatticeBuilder::with_stub_work();
        let account = PrivateKey::from(42);

        let send1 = builder.genesis().send(&account, 1);
        let send2 = builder.genesis().send(&account, 1);
        let open = builder.account(&account).receive(&send1);
        let receive = builder.account(&account).receive(&send2);
        ledger.process_one(&send1).unwrap();
        ledger.process_one(&send2).unwrap();
        ledger.process_one(&open).unwrap();
        ledger.process_one(&receive).unwrap();
        ledger.confirm(open.hash());

        assert!(!activate(&scheduler, account.account()));

        while scheduler.run_one() {}

        let optimistic_count = aec.count_by_behavior(ElectionBehavior::Optimistic);
        assert_eq!(optimistic_count, 0, "should not schedule any election");
    }

    #[test]
    fn schedules_elections_for_multiple_unconfirmed_accounts() {
        let aec = Arc::new(AecService::new_null());
        let ledger = Arc::new(Ledger::new_null());
        let scheduler = make_scheduler_with(aec.clone(), ledger.clone());

        let mut builder = UnsavedBlockLatticeBuilder::with_stub_work();
        let account1 = PrivateKey::from(1);
        let account2 = PrivateKey::from(2);

        let count = TEST_GAP_THRESHOLD as usize + 1;
        let mut last1 = None;
        let mut last2 = None;
        let sends1: Vec<_> = (0..count)
            .map(|_| builder.genesis().send(&account1, 1))
            .collect();
        let receives1: Vec<_> = sends1
            .iter()
            .map(|s| builder.account(&account1).receive(s))
            .collect();
        let sends2: Vec<_> = (0..count)
            .map(|_| builder.genesis().send(&account2, 1))
            .collect();
        let receives2: Vec<_> = sends2
            .iter()
            .map(|s| builder.account(&account2).receive(s))
            .collect();
        for s in &sends1 {
            ledger.process_one(s).unwrap();
        }
        for r in &receives1 {
            last1 = Some(ledger.process_one(r).unwrap());
        }
        for s in &sends2 {
            ledger.process_one(s).unwrap();
        }
        for r in &receives2 {
            last2 = Some(ledger.process_one(r).unwrap());
        }

        assert!(activate(&scheduler, account1.account()));
        assert!(activate(&scheduler, account2.account()));

        while scheduler.run_one() {}

        let optimistic_count = aec.count_by_behavior(ElectionBehavior::Optimistic);

        assert_eq!(
            optimistic_count, 2,
            "should schedule elections for both accounts"
        );
        assert!(aec.is_active_hash(&last1.unwrap().hash()));
        assert!(aec.is_active_hash(&last2.unwrap().hash()));
    }

    fn make_scheduler() -> OptimisticScheduler {
        OptimisticScheduler::new(
            test_params(),
            Arc::new(AecService::new_null()),
            Ledger::new_null().into(),
            ConfirmingSet::new_null().into(),
            SteadyClock::new_null().into(),
        )
    }

    fn make_scheduler_with(aec: Arc<AecService>, ledger: Arc<Ledger>) -> OptimisticScheduler {
        OptimisticScheduler::new(
            test_params(),
            aec,
            ledger,
            ConfirmingSet::new_null().into(),
            SteadyClock::new_null().into(),
        )
    }

    fn test_params() -> OptimisticSchedulerParams {
        OptimisticSchedulerParams {
            gap_threshold: TEST_GAP_THRESHOLD,
            max_candidates: 1024,
            max_elections: 10,
            activation_delay: Duration::ZERO,
        }
    }

    fn activate(scheduler: &OptimisticScheduler, account: Account) -> bool {
        let ledger = &scheduler.ledger;
        let block_count = ledger.any().get_account(&account).unwrap().block_count;
        let conf_height = ledger
            .confirmed()
            .get_conf_info(&account)
            .map(|i| i.height)
            .unwrap_or(0);
        scheduler.activate(&account, block_count, conf_height)
    }

    const TEST_GAP_THRESHOLD: u64 = 16;
}
