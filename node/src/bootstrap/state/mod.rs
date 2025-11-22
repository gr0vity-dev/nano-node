mod account_ack_processor;
pub(crate) mod block_ack_processor;
pub(crate) mod block_queue;
pub(crate) mod bootstrap_logic;
mod candidate_accounts;
mod frontier_scan;
pub mod frontiers_processor;
mod peer_scoring;
mod running_query;
mod running_query_container;

pub use bootstrap_logic::{BootstrapLogic, BootstrapLogicSnapshot};
pub use candidate_accounts::{BlockingEntry, CandidateAccountsSnapshot, Priority, PriorityResult};
pub(crate) use candidate_accounts::{
    CandidateAccounts, CandidateAccountsConfig, PriorityDownResult, PriorityUpResult,
};
pub(crate) use frontier_scan::FrontierScan;
pub use frontier_scan::{FrontierHeadInfo, FrontierScanConfig};
pub(crate) use peer_scoring::PeerScoring;
pub(crate) use running_query::*;
pub(crate) use running_query_container::*;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum VerifyResult {
    Ok,
    NothingNew,
    Invalid,
}
