use crate::bootstrap::{
    FrontierHeadInfo, FrontierScanConfig,
    state::{CandidateAccounts, FrontierScan, RunningQuery, VerifyResult},
};
use rsnano_nullable_clock::Timestamp;
use rsnano_types::{Account, Frontier};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{StatsCollection, StatsSource},
};
use std::collections::VecDeque;

pub struct FrontiersProcessor {
    pub(crate) frontier_scan: FrontierScan,
    pub stats: FrontiersStats,

    /// Frontiers that were received from other nodes and that we need to check against our ledger
    frontiers_to_check: VecDeque<Vec<Frontier>>,
    frontier_checker_overfill: bool,
    pub last_outdated_accounts: VecDeque<Account>,
}

impl FrontiersProcessor {
    pub fn new(config: FrontierScanConfig) -> Self {
        Self {
            frontier_scan: FrontierScan::new(config),
            stats: Default::default(),
            frontiers_to_check: Default::default(),
            frontier_checker_overfill: false,
            last_outdated_accounts: Default::default(),
        }
    }

    pub fn heads(&self) -> Vec<FrontierHeadInfo> {
        self.frontier_scan.heads()
    }

    pub fn set_frontier_checker_overfill(&mut self, overfill: bool) {
        self.frontier_checker_overfill = overfill;
    }

    pub fn frontier_checker_overfill(&self) -> bool {
        self.frontier_checker_overfill
    }

    pub fn next(&mut self, now: Timestamp) -> Account {
        self.frontier_scan.next(now)
    }

    /// Returns true if the frontiers were valid
    pub(crate) fn process(&mut self, query: &RunningQuery, frontiers: Vec<Frontier>) -> bool {
        self.stats.processed_responses += 1;

        let valid_frontiers = match query.verify_frontiers(&frontiers) {
            VerifyResult::Ok => {
                self.stats.verified += 1;
                self.frontier_scan.process(query.start.into(), &frontiers);
                self.frontiers_to_check.push_back(frontiers);
                true
            }
            VerifyResult::NothingNew => {
                self.stats.nothing_new += 1;
                // OK, but nothing to do
                true
            }
            VerifyResult::Invalid => {
                self.stats.invalid += 1;
                false
            }
        };
        valid_frontiers
    }

    pub fn pop_frontiers_to_check(&mut self) -> Option<Vec<Frontier>> {
        self.frontiers_to_check.pop_front()
    }

    pub fn frontiers_processed(
        &mut self,
        outdated: &OutdatedAccounts,
        candidates: &mut CandidateAccounts,
    ) {
        self.stats.processed_frontiers += outdated.frontiers_received as u64;
        self.stats.outdated_accounts_found += outdated.accounts.len() as u64;

        for account in &outdated.accounts {
            candidates.priority_up(account);

            self.last_outdated_accounts.push_back(*account);
            if self.last_outdated_accounts.len() > 20 {
                self.last_outdated_accounts.pop_front();
            }
        }
    }
}

impl Default for FrontiersProcessor {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl StatsSource for FrontiersProcessor {
    fn collect_stats(&self, result: &mut StatsCollection) {
        self.stats.collect_stats(result);
    }
}

impl ContainerInfoProvider for FrontiersProcessor {
    fn container_info(&self) -> ContainerInfo {
        self.frontier_scan.container_info()
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct OutdatedAccounts {
    pub accounts: Vec<Account>,
    /// Accounts that exist but are outdated
    pub outdated: usize,
    /// Accounts that don't exist but have pending blocks in the ledger
    pub pending: usize,
    /// Total count of received frontiers
    pub frontiers_received: usize,
}

#[derive(Default, Clone, Debug)]
pub struct FrontiersStats {
    pub processed_responses: u64,
    pub processed_frontiers: u64,
    pub verified: u64,
    pub nothing_new: u64,
    pub invalid: u64,
    pub outdated_accounts_found: u64,
}

impl StatsSource for FrontiersStats {
    fn collect_stats(&self, result: &mut StatsCollection) {
        result.insert("bootstrap_verify_frontiers", "ok", self.verified);
        result.insert(
            "bootstrap_verify_frontiers",
            "nothing_new",
            self.nothing_new,
        );
        result.insert("bootstrap_verify_frontiers", "invalid", self.invalid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::state::{QuerySource, QueryType};

    #[test]
    fn empty_frontiers() {
        let mut processor = FrontiersProcessor::default();
        let query = running_query();

        let success = processor.process(&query, Vec::new());

        assert!(success);
        assert_eq!(processor.stats.processed_responses, 1);
        assert_eq!(processor.stats.verified, 0);
        assert_eq!(processor.stats.nothing_new, 1);
    }

    #[test]
    fn update_account_ranges() {
        let mut processor = FrontiersProcessor::default();
        let query = running_query();

        let success = processor.process(&query, vec![Frontier::new_test_instance()]);

        assert!(success);
        assert_eq!(processor.frontier_scan.total_requests_completed(), 1);
        assert_eq!(processor.stats.processed_responses, 1);
        assert_eq!(processor.stats.verified, 1);
    }

    #[test]
    fn invalid_frontiers() {
        let mut processor = FrontiersProcessor::default();
        let query = running_query();

        let frontiers = vec![
            Frontier::new(3.into(), 100.into()),
            Frontier::new(1.into(), 200.into()), // descending order is invalid!
        ];

        let success = processor.process(&query, frontiers);

        assert!(!success);
        assert_eq!(processor.frontier_scan.total_requests_completed(), 0);
        assert_eq!(processor.stats.processed_responses, 1);
        assert_eq!(processor.stats.invalid, 1);
    }

    fn running_query() -> RunningQuery {
        RunningQuery {
            source: QuerySource::Frontiers,
            query_type: QueryType::Frontiers,
            start: 1.into(),
            ..RunningQuery::new_test_instance()
        }
    }
}
