use std::collections::VecDeque;

use rsnano_node::{bootstrap::{FrontierHeadInfo, state::{BootstrapLogicSnapshot, frontiers_processor::FrontiersStats}}, utils::RateCalculator};
use rsnano_nullable_clock::Timestamp;
use rsnano_types::Account;

#[derive(Default)]
pub(crate) struct FrontierScanInfo {
    frontiers_rate: RateCalculator,
    outdated_rate: RateCalculator,
    pub frontier_heads: Vec<FrontierHeadInfo>,
    pub frontiers_total: u64,
    pub outdated_total: u64,
    pub outdated_accounts: VecDeque<Account>,
}

impl FrontierScanInfo {
    pub(crate) fn update(&mut self, state: &BootstrapLogicSnapshot, now: Timestamp) {
        self.update_counters(&state.frontiers_stats, now);
        self.frontier_heads = state.frontier_heads.clone();
        self.outdated_accounts = state.last_outdated_accounts.clone();
    }

    fn update_counters(&mut self, stats: &FrontiersStats, now: Timestamp) {
        self.frontiers_rate.sample(stats.processed_frontiers, now);
        self.outdated_rate
            .sample(stats.outdated_accounts_found, now);
        self.frontiers_total = stats.processed_frontiers;
        self.outdated_total = stats.outdated_accounts_found;
    }

    pub(crate) fn frontiers_rate(&self) -> i64 {
        self.frontiers_rate.rate()
    }

    pub(crate) fn outdated_rate(&self) -> i64 {
        self.outdated_rate.rate()
    }
}
