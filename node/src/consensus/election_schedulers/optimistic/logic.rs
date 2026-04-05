use std::cmp::min;

use rsnano_nullable_clock::Timestamp;
use rsnano_types::Account;

use super::{candidate_queue::CandidateQueue, config::OptimisticSchedulerParams};

/// Pure scheduling logic — no infrastructure dependencies.
/// Encodes optimistic ranking and timing rules without owning runtime state.
pub struct OptimisticSchedulerLogic {
    params: OptimisticSchedulerParams,
}

impl OptimisticSchedulerLogic {
    pub fn new(params: OptimisticSchedulerParams) -> Self {
        Self { params }
    }

    /// Attempts to enqueue the account as an optimistic candidate.
    /// Returns true if the account was newly added.
    pub fn try_activate(
        &self,
        candidates: &mut CandidateQueue,
        account: &Account,
        block_count: u64,
        confirmation_height: u64,
        now: Timestamp,
    ) -> bool {
        let gap = self.get_gap(block_count, confirmation_height);
        if gap < self.params.gap_threshold {
            return false;
        }

        let already_queued = candidates.contains(account);

        if !already_queued && candidates.len() >= self.params.max_candidates {
            // Evict the lowest gap entry if the new one has a strictly higher gap
            let Some(min_gap) = candidates.min_gap() else {
                return false;
            };
            if gap <= min_gap {
                return false;
            }
            candidates.pop_lowest_gap_entry();
        }

        candidates.insert(*account, now, gap);
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

    pub fn activation_delay(&self) -> std::time::Duration {
        self.params.activation_delay
    }

    pub fn pop_candidate(
        &self,
        candidates: &mut CandidateQueue,
        now: Timestamp,
    ) -> Option<Account> {
        candidates.pop_first(now - self.params.activation_delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /*
     * try_activate accepts
     */

    #[test]
    fn try_activate_adds_candidate_when_gap_above_threshold() {
        let logic = OptimisticSchedulerLogic::new(make_params(32, 1024));
        let mut candidates = CandidateQueue::default();
        assert!(logic.try_activate(&mut candidates, &Account::from(1), 100, 10, now()));
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn try_activate_adds_when_account_fully_unconfirmed() {
        let logic = OptimisticSchedulerLogic::new(make_params(32, 1024));
        let mut candidates = CandidateQueue::default();
        assert!(logic.try_activate(&mut candidates, &Account::from(1), 100, 0, now()));
        assert_eq!(candidates.len(), 1);
    }

    /*
     * try_activate rejects
     */

    #[test]
    fn try_activate_updates_gap_on_duplicate() {
        let logic = OptimisticSchedulerLogic::new(make_params(32, 1024));
        let mut candidates = CandidateQueue::default();
        let account = Account::from(1);
        assert!(logic.try_activate(&mut candidates, &account, 100, 0, now())); // newly added
        assert!(!logic.try_activate(&mut candidates, &account, 200, 0, now())); // gap updated, not newly added
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn try_activate_rejects_when_full_and_gap_not_higher() {
        let logic = OptimisticSchedulerLogic::new(make_params(32, 2));
        let mut candidates = CandidateQueue::default();
        assert!(logic.try_activate(&mut candidates, &Account::from(1), 100, 0, now())); // gap = 32
        assert!(logic.try_activate(&mut candidates, &Account::from(2), 100, 0, now())); // gap = 32
        assert!(!logic.try_activate(&mut candidates, &Account::from(3), 100, 0, now())); // gap = 32, not higher
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn try_activate_evicts_lowest_gap_when_full_and_new_gap_is_higher() {
        let logic = OptimisticSchedulerLogic::new(make_params(32, 2));
        let mut candidates = CandidateQueue::default();
        let low = Account::from(1);
        let mid = Account::from(2);
        let high = Account::from(3);
        logic.try_activate(&mut candidates, &low, 132, 100, now()); // gap = 32 (threshold, lowest)
        logic.try_activate(&mut candidates, &mid, 164, 100, now()); // gap = 64
        logic.try_activate(&mut candidates, &high, 200, 100, now()); // gap = 100, evicts low

        assert_eq!(candidates.len(), 2);
        assert_eq!(logic.pop_candidate(&mut candidates, now()), Some(high));
        assert_eq!(logic.pop_candidate(&mut candidates, now()), Some(mid));
    }

    #[test]
    fn try_activate_rejects_when_gap_too_small() {
        let logic = OptimisticSchedulerLogic::new(make_params(32, 1024));
        let mut candidates = CandidateQueue::default();
        assert!(!logic.try_activate(&mut candidates, &Account::from(1), 100, 80, now())); // gap = 20, below threshold
        assert_eq!(candidates.len(), 0);
    }

    /*
     * Misc
     */

    #[test]
    fn pop_candidate_returns_highest_gap_first() {
        let logic = OptimisticSchedulerLogic::new(make_params(32, 1024));
        let mut candidates = CandidateQueue::default();
        let a = Account::from(1);
        let b = Account::from(2);
        logic.try_activate(&mut candidates, &a, 100, 0, now()); // gap = 100
        logic.try_activate(&mut candidates, &b, 200, 0, now()); // gap = 200

        let first = logic.pop_candidate(&mut candidates, now()).unwrap();
        assert_eq!(first, b);
        let second = logic.pop_candidate(&mut candidates, now()).unwrap();
        assert_eq!(second, a);
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
}
