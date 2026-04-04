use std::cmp::min;
use std::time::Duration;

use rsnano_nullable_clock::Timestamp;
use rsnano_types::Account;

use super::{candidate_queue::CandidateQueue, config::OptimisticSchedulerParams};
use rsnano_utils::container_info::{ContainerInfo, ContainerInfoProvider};

/// Pure scheduling logic — no infrastructure dependencies.
/// Manages the candidate queue and decides when activation is appropriate.
pub struct OptimisticSchedulerLogic {
    stopped: bool,
    params: OptimisticSchedulerParams,
    candidates: CandidateQueue,
}

impl OptimisticSchedulerLogic {
    pub fn new(params: OptimisticSchedulerParams) -> Self {
        Self {
            stopped: false,
            params,
            candidates: CandidateQueue::default(),
        }
    }

    pub fn stopped(&self) -> bool {
        self.stopped
    }

    pub fn stop(&mut self) {
        self.stopped = true;
    }

    /// Attempts to enqueue the account as an optimistic candidate.
    /// Returns true if the account was newly added.
    pub fn try_activate(
        &mut self,
        account: &Account,
        block_count: u64,
        confirmation_height: u64,
        now: Timestamp,
    ) -> bool {
        if self.stopped() {
            return false;
        }

        let gap = self.get_gap(block_count, confirmation_height);
        if gap < self.params.gap_threshold {
            return false;
        }

        let already_queued = self.candidates.contains(account);

        if !already_queued && self.candidates.len() >= self.params.max_candidates {
            // Evict the lowest gap entry if the new one has a strictly higher gap
            let Some(min_gap) = self.candidates.min_gap() else {
                return false;
            };
            if gap <= min_gap {
                return false;
            }
            self.candidates.pop_lowest_gap_entry();
        }

        self.candidates.insert(*account, now, gap);
        !already_queued
    }

    fn get_gap(&self, block_count: u64, confirmation_height: u64) -> u64 {
        block_count.saturating_sub(confirmation_height)
    }

    pub fn has_vacancy(&self, optimistic_count: usize, aec_vacancy: i64) -> bool {
        let vacancy = min(
            self.params.max_elections as i64 - optimistic_count as i64,
            aec_vacancy,
        );
        vacancy > 0
    }

    pub fn pop_candidate(&mut self, now: Timestamp) -> Option<Account> {
        self.candidates
            .pop_first(now - self.params.activation_delay)
    }

    pub fn has_ready_candidate(&self, now: Timestamp) -> bool {
        self.next_activation_deadline()
            .is_some_and(|deadline| deadline <= now)
    }

    pub fn next_activation_delay(&self, now: Timestamp) -> Option<Duration> {
        self.next_activation_deadline().map(|deadline| {
            if deadline <= now {
                Duration::ZERO
            } else {
                deadline - now
            }
        })
    }

    pub fn max_elections(&self) -> usize {
        self.params.max_elections
    }

    fn next_activation_deadline(&self) -> Option<Timestamp> {
        self.candidates
            .oldest_timestamp()
            .map(|timestamp| timestamp + self.params.activation_delay)
    }

    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }
}

impl ContainerInfoProvider for OptimisticSchedulerLogic {
    fn container_info(&self) -> ContainerInfo {
        [("candidates", self.candidate_count(), 1)].into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn stopped_initially_false() {
        assert!(!OptimisticSchedulerLogic::new(make_params(32, 1024)).stopped());
    }

    #[test]
    fn stop_sets_flag() {
        let mut logic = OptimisticSchedulerLogic::new(make_params(32, 1024));
        logic.stop();
        assert!(logic.stopped());
    }

    /*
     * try_activate accepts
     */

    #[test]
    fn try_activate_adds_candidate_when_gap_above_threshold() {
        let mut logic = OptimisticSchedulerLogic::new(make_params(32, 1024));
        assert!(logic.try_activate(&Account::from(1), 100, 10, now()));
        assert_eq!(logic.candidate_count(), 1);
    }

    #[test]
    fn try_activate_adds_when_account_fully_unconfirmed() {
        let mut logic = OptimisticSchedulerLogic::new(make_params(32, 1024));
        assert!(logic.try_activate(&Account::from(1), 100, 0, now()));
        assert_eq!(logic.candidate_count(), 1);
    }

    /*
     * try_activate rejects
     */

    #[test]
    fn try_activate_updates_gap_on_duplicate() {
        let mut logic = OptimisticSchedulerLogic::new(make_params(32, 1024));
        let account = Account::from(1);
        assert!(logic.try_activate(&account, 100, 0, now())); // newly added
        assert!(!logic.try_activate(&account, 200, 0, now())); // gap updated, not newly added
        assert_eq!(logic.candidate_count(), 1);
    }

    #[test]
    fn try_activate_rejects_when_full_and_gap_not_higher() {
        let mut logic = OptimisticSchedulerLogic::new(make_params(32, 2));
        assert!(logic.try_activate(&Account::from(1), 100, 0, now())); // gap = 32
        assert!(logic.try_activate(&Account::from(2), 100, 0, now())); // gap = 32
        assert!(!logic.try_activate(&Account::from(3), 100, 0, now())); // gap = 32, not higher
        assert_eq!(logic.candidate_count(), 2);
    }

    #[test]
    fn try_activate_evicts_lowest_gap_when_full_and_new_gap_is_higher() {
        let mut logic = OptimisticSchedulerLogic::new(make_params(32, 2));
        let low = Account::from(1);
        let mid = Account::from(2);
        let high = Account::from(3);
        logic.try_activate(&low, 132, 100, now()); // gap = 32 (threshold, lowest)
        logic.try_activate(&mid, 164, 100, now()); // gap = 64
        logic.try_activate(&high, 200, 100, now()); // gap = 100, evicts low

        assert_eq!(logic.candidate_count(), 2);
        assert_eq!(logic.pop_candidate(now()), Some(high));
        assert_eq!(logic.pop_candidate(now()), Some(mid));
    }

    #[test]
    fn try_activate_rejects_when_gap_too_small() {
        let mut logic = OptimisticSchedulerLogic::new(make_params(32, 1024));
        assert!(!logic.try_activate(&Account::from(1), 100, 80, now())); // gap = 20, below threshold
        assert_eq!(logic.candidate_count(), 0);
    }

    /*
     * Misc
     */

    #[test]
    fn pop_candidate_returns_highest_gap_first() {
        let mut logic = OptimisticSchedulerLogic::new(make_params(32, 1024));
        let a = Account::from(1);
        let b = Account::from(2);
        logic.try_activate(&a, 100, 0, now()); // gap = 100
        logic.try_activate(&b, 200, 0, now()); // gap = 200

        let first = logic.pop_candidate(now()).unwrap();
        assert_eq!(first, b);
        let second = logic.pop_candidate(now()).unwrap();
        assert_eq!(second, a);
    }

    #[test]
    fn has_ready_candidate_after_activation_delay_elapsed() {
        let mut logic = OptimisticSchedulerLogic::new(make_delayed_params(Duration::from_secs(5)));
        let inserted_at = now();
        logic.try_activate(&Account::from(1), 100, 0, inserted_at);

        assert!(!logic.has_ready_candidate(inserted_at));
        assert!(logic.has_ready_candidate(inserted_at + Duration::from_secs(5)));
    }

    #[test]
    fn next_activation_delay_uses_oldest_candidate() {
        let mut logic = OptimisticSchedulerLogic::new(make_delayed_params(Duration::from_secs(5)));
        let inserted_at = now();
        logic.try_activate(&Account::from(1), 100, 0, inserted_at);

        assert_eq!(
            logic.next_activation_delay(inserted_at + Duration::from_secs(2)),
            Some(Duration::from_secs(3))
        );
    }

    /* Test helpers */

    fn now() -> Timestamp {
        Timestamp::new_test_instance()
    }

    fn make_params(gap_threshold: u64, max_candidates: usize) -> OptimisticSchedulerParams {
        OptimisticSchedulerParams {
            gap_threshold,
            max_candidates,
            max_elections: 10,
            activation_delay: Duration::ZERO,
        }
    }

    fn make_delayed_params(activation_delay: Duration) -> OptimisticSchedulerParams {
        OptimisticSchedulerParams {
            gap_threshold: 32,
            max_candidates: 1024,
            max_elections: 10,
            activation_delay,
        }
    }
}
