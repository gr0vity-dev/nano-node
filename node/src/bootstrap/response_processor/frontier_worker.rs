use std::sync::{Arc, Mutex};

use rsnano_ledger::Ledger;
use rsnano_types::Frontier;
use rsnano_utils::stats::{DetailType, StatType, Stats};

use super::frontier_checker::FrontierChecker;
use crate::bootstrap::state::{BootstrapLogic, frontiers_processor::OutdatedAccounts};

/// Handles received frontiers
pub(crate) struct FrontierWorker<'a> {
    stats: &'a Stats,
    state: &'a Mutex<BootstrapLogic>,
    ledger: Arc<Ledger>,
}

impl<'a> FrontierWorker<'a> {
    pub(crate) fn new(
        ledger: Arc<Ledger>,
        stats: &'a Stats,
        state: &'a Mutex<BootstrapLogic>,
    ) -> Self {
        Self {
            stats,
            state,
            ledger,
        }
    }

    pub fn process(&mut self, frontiers: Vec<Frontier>) {
        let any = self.ledger.any();
        let mut checker = FrontierChecker::new(&any);
        let outdated = checker.get_outdated_accounts(&frontiers);
        self.update_stats(&frontiers, &outdated);
        self.state.lock().unwrap().frontiers_processed(&outdated);
    }

    fn update_stats(&self, frontiers: &[Frontier], outdated: &OutdatedAccounts) {
        self.stats.add(
            StatType::BootstrapFrontiers,
            DetailType::Processed,
            frontiers.len() as u64,
        );
        self.stats.add(
            StatType::BootstrapFrontiers,
            DetailType::Prioritized,
            outdated.accounts.len() as u64,
        );
        self.stats.add(
            StatType::BootstrapFrontiers,
            DetailType::Outdated,
            outdated.outdated as u64,
        );
        self.stats.add(
            StatType::BootstrapFrontiers,
            DetailType::Pending,
            outdated.pending as u64,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::state::CandidateAccounts;
    use crate::ledger_factory::default_ledger_store_factory;
    use rsnano_ledger::Ledger;
    use rsnano_types::{Account, AccountInfo, BlockHash};

    #[test]
    fn empty() {
        let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));
        let stats = Stats::default();
        let state = Mutex::new(BootstrapLogic::default());
        let mut worker = FrontierWorker::new(ledger, &stats, &state);

        worker.process(Vec::new());

        assert_eq!(state.lock().unwrap().candidate_accounts.priority_len(), 0);
    }

    #[test]
    fn prioritize_one_account() {
        let account = Account::from(1);
        let ledger = Arc::new(
            Ledger::new_null_builder(default_ledger_store_factory())
                .account_info(
                    &account,
                    &AccountInfo {
                        head: BlockHash::from(2),
                        ..Default::default()
                    },
                )
                .finish(),
        );
        let stats = Stats::default();
        let state = Mutex::new(BootstrapLogic::default());
        let mut worker = FrontierWorker::new(ledger, &stats, &state);

        worker.process(vec![Frontier::new(account, BlockHash::from(3))]);

        let guard = state.lock().unwrap();
        assert_eq!(guard.candidate_accounts.priority_len(), 1);
        assert_eq!(
            guard.candidate_accounts.priority(&account),
            CandidateAccounts::PRIORITY_INITIAL
        );
        assert_eq!(guard.frontiers_processor.stats.outdated_accounts_found, 1);
        assert_eq!(guard.frontiers_processor.stats.processed_frontiers, 1);
    }
}
