mod candidate_queue;
mod config;
mod logic;
mod stats;

pub use config::OptimisticSchedulerParams;
pub(crate) use logic::OptimisticSchedulerLogic;
pub(crate) use stats::OptimisticSchedulerStats;

#[cfg(test)]
mod tests {
    use super::OptimisticSchedulerParams;
    use crate::{
        cementation::ConfirmingSet,
        consensus::{AecService, election::ElectionBehavior, election_schedulers::candidate_coordinator::CandidateCoordinator, election_schedulers::priority::PriorityBucketConfig},
    };
    use rsnano_ledger::{AnySet, ConfirmedSet, Ledger, LedgerSet, test_helpers::UnsavedBlockLatticeBuilder};
    use rsnano_nullable_clock::SteadyClock;
    use rsnano_types::PrivateKey;
    use rsnano_utils::stats::Stats;
    use std::{sync::Arc, time::Duration};

    #[test]
    fn schedules_election_when_over_gap_threshold() {
        let aec = Arc::new(AecService::new_null());
        let ledger = Arc::new(Ledger::new_null());
        let coordinator = make_coordinator(aec.clone(), ledger.clone());

        let mut builder = UnsavedBlockLatticeBuilder::with_stub_work();
        for _ in 0..TEST_GAP_THRESHOLD {
            let block = builder.genesis().send(1, 1);
            ledger.process_one(&block).unwrap();
        }

        assert!(activate(&coordinator, ledger.genesis().account()));

        coordinator.run_optimistic();

        let optimistic_count = aec.count_by_behavior(ElectionBehavior::Optimistic);

        assert_eq!(optimistic_count, 1, "should schedule the election");
        assert_eq!(coordinator.optimistic_candidate_count(), 0);
    }

    #[test]
    fn schedules_election_when_account_is_unconfirmed() {
        let aec = Arc::new(AecService::new_null());
        let ledger = Arc::new(Ledger::new_null());
        let coordinator = make_coordinator(aec.clone(), ledger.clone());

        let mut builder = UnsavedBlockLatticeBuilder::with_stub_work();
        let unconf_account = PrivateKey::from(42);

        // Build enough blocks to exceed TEST_GAP_THRESHOLD, all unconfirmed
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

        assert!(activate(&coordinator, unconf_account.account()));

        coordinator.run_optimistic();

        let optimistic_count = aec.count_by_behavior(ElectionBehavior::Optimistic);

        assert_eq!(optimistic_count, 1, "should schedule the election");
        assert!(aec.is_active_hash(&head.hash()));
    }

    #[test]
    fn does_not_schedule_when_gap_is_under_threshold() {
        let aec = Arc::new(AecService::new_null());
        let ledger = Arc::new(Ledger::new_null());
        let coordinator = make_coordinator(aec.clone(), ledger.clone());

        let mut builder = UnsavedBlockLatticeBuilder::with_stub_work();
        let account = PrivateKey::from(42);

        // Two blocks in account chain: open + receive
        let send1 = builder.genesis().send(&account, 1);
        let send2 = builder.genesis().send(&account, 1);
        let open = builder.account(&account).receive(&send1);
        let receive = builder.account(&account).receive(&send2);
        ledger.process_one(&send1).unwrap();
        ledger.process_one(&send2).unwrap();
        ledger.process_one(&open).unwrap();
        ledger.process_one(&receive).unwrap();
        // Confirm up to open, leaving gap of 1 (well below TEST_GAP_THRESHOLD)
        ledger.confirm(open.hash());

        // activate should reject: gap = 2 - 1 = 1 < TEST_GAP_THRESHOLD and conf_height > 0
        assert!(!activate(&coordinator, account.account()));

        coordinator.run_optimistic();

        let optimistic_count = aec.count_by_behavior(ElectionBehavior::Optimistic);
        assert_eq!(optimistic_count, 0, "should not schedule any election");
    }

    #[test]
    fn schedules_elections_for_multiple_unconfirmed_accounts() {
        let aec = Arc::new(AecService::new_null());
        let ledger = Arc::new(Ledger::new_null());
        let coordinator = make_coordinator(aec.clone(), ledger.clone());

        let mut builder = UnsavedBlockLatticeBuilder::with_stub_work();
        let account1 = PrivateKey::from(1);
        let account2 = PrivateKey::from(2);

        // Build enough blocks per account to exceed TEST_GAP_THRESHOLD, all unconfirmed
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

        assert!(activate(&coordinator, account1.account()));
        assert!(activate(&coordinator, account2.account()));

        coordinator.run_optimistic();

        let optimistic_count = aec.count_by_behavior(ElectionBehavior::Optimistic);

        assert_eq!(
            optimistic_count, 2,
            "should schedule elections for both accounts"
        );
        assert!(aec.is_active_hash(&last1.unwrap().hash()));
        assert!(aec.is_active_hash(&last2.unwrap().hash()));
    }

    #[test]
    fn delays_optimistic_activation_until_candidate_ages() {
        let aec = Arc::new(AecService::new_null());
        let ledger = Arc::new(Ledger::new_null());
        let clock = Arc::new(SteadyClock::new_null());
        let coordinator = make_coordinator_with(
            aec.clone(),
            ledger.clone(),
            Arc::new(ConfirmingSet::new_null()),
            clock.clone(),
            OptimisticSchedulerParams {
                activation_delay: Duration::from_secs(1),
                ..test_params()
            },
        );

        let mut builder = UnsavedBlockLatticeBuilder::with_stub_work();
        for _ in 0..TEST_GAP_THRESHOLD {
            let block = builder.genesis().send(1, 1);
            ledger.process_one(&block).unwrap();
        }
        let head = ledger.any().account_head(&ledger.genesis().account()).unwrap();

        assert!(activate(&coordinator, ledger.genesis().account()));

        coordinator.run_optimistic();
        assert_eq!(aec.count_by_behavior(ElectionBehavior::Optimistic), 0);

        clock.advance(Duration::from_secs(1));
        coordinator.run_optimistic();
        assert_eq!(aec.count_by_behavior(ElectionBehavior::Optimistic), 1);
        assert!(aec.is_active_hash(&head));
    }

    /* Test helpers */

    fn make_coordinator(aec: Arc<AecService>, ledger: Arc<Ledger>) -> CandidateCoordinator {
        make_coordinator_with(
            aec,
            ledger,
            Arc::new(ConfirmingSet::new_null()),
            Arc::new(SteadyClock::new_null()),
            test_params(),
        )
    }

    fn make_coordinator_with(
        aec: Arc<AecService>,
        ledger: Arc<Ledger>,
        confirming_set: Arc<ConfirmingSet>,
        clock: Arc<SteadyClock>,
        params: OptimisticSchedulerParams,
    ) -> CandidateCoordinator {
        CandidateCoordinator::new(
            PriorityBucketConfig::default(),
            true,
            params,
            true,
            Arc::new(Stats::default()),
            aec,
            ledger,
            confirming_set,
            clock,
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

    fn activate(coordinator: &CandidateCoordinator, account: rsnano_types::Account) -> bool {
        let ledger = coordinator.ledger_for_tests();
        let block_count = ledger.any().get_account(&account).unwrap().block_count;
        let conf_height = ledger
            .confirmed()
            .get_conf_info(&account)
            .map(|i| i.height)
            .unwrap_or(0);
        coordinator.activate_optimistic(&account, block_count, conf_height)
    }

    const TEST_GAP_THRESHOLD: u64 = 16;
}
