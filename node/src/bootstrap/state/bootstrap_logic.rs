use std::{collections::VecDeque, sync::Arc, time::Duration};

use rsnano_messages::{AscPullAck, AscPullAckType, AscPullReqType};
use rsnano_network::{Channel, ChannelId};
use rsnano_nullable_clock::Timestamp;
use rsnano_types::{Account, BlockHash};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{StatsCollection, StatsSource},
};

use super::{
    CandidateAccounts, CandidateAccountsSnapshot, PeerScoring, PriorityResult, RunningQueryContainer,
    running_query::QuerySource,
};
use crate::bootstrap::{
    AscPullQuerySpec, BootstrapConfig,
    state::{
        QueryType, RunningQuery,
        account_ack_processor::AccountAckProcessor,
        block_ack_processor::BlockAckProcessor,
        frontiers_processor::{FrontiersProcessor, OutdatedAccounts},
    },
};
use crate::bootstrap::state::frontier_scan::FrontierHeadInfo;
use super::frontiers_processor::FrontiersStats;

pub struct BootstrapLogic {
    pub candidate_accounts: CandidateAccounts,
    pub(crate) scoring: PeerScoring,
    pub(crate) running_queries: RunningQueryContainer,
    pub(crate) stopped: bool,
    account_ack_processor: AccountAckProcessor,
    pub frontiers_processor: FrontiersProcessor,
    pub block_ack_processor: BlockAckProcessor,

    response_blocks: u64,
    response_account: u64,
    response_frontiers: u64,
}

pub struct BootstrapLogicSnapshot {
    pub candidate_accounts: CandidateAccountsSnapshot,
    pub frontiers_stats: FrontiersStats,
    pub frontier_heads: Vec<FrontierHeadInfo>,
    pub last_outdated_accounts: VecDeque<Account>,
}

impl From<&BootstrapLogic> for BootstrapLogicSnapshot {
    fn from(logic: &BootstrapLogic) -> Self {
        let stats = &logic.frontiers_processor.stats;
        Self {
            candidate_accounts: logic.candidate_accounts.snapshot(),
            frontiers_stats: FrontiersStats {
                processed_responses: stats.processed_responses,
                processed_frontiers: stats.processed_frontiers,
                verified: stats.verified,
                nothing_new: stats.nothing_new,
                invalid: stats.invalid,
                outdated_accounts_found: stats.outdated_accounts_found,
            },
            frontier_heads: logic.frontiers_processor.heads(),
            last_outdated_accounts: logic
                .frontiers_processor
                .last_outdated_accounts
                .clone(),
        }
    }
}

impl BootstrapLogic {
    pub fn new(config: BootstrapConfig) -> Self {
        let mut scoring = PeerScoring::new();
        scoring.set_channel_limit(config.channel_limit);

        Self {
            candidate_accounts: CandidateAccounts::new(config.candidate_accounts.clone()),
            scoring,
            running_queries: RunningQueryContainer::default(),
            stopped: false,
            account_ack_processor: Default::default(),
            frontiers_processor: FrontiersProcessor::new(config.frontier_scan.clone()),
            block_ack_processor: Default::default(),
            response_blocks: 0,
            response_account: 0,
            response_frontiers: 0,
        }
    }

    pub fn next_priority(&mut self, now: Timestamp) -> PriorityResult {
        let next = self.candidate_accounts.next_priority(now, |account| {
            !self
                .block_ack_processor
                .block_queue
                .contains_account(account)
                && self
                    .running_queries
                    .count_by_account(account, QuerySource::Priority)
                    == 0
        });

        if next.account.is_zero() {
            return Default::default();
        }

        next
    }

    pub fn next_blocking_query(
        &self,
        query_id: u64,
        channel: &Arc<Channel>,
    ) -> Option<AscPullQuerySpec> {
        let next = self.next_blocking();
        if next.is_zero() {
            return None;
        }
        Some(AscPullQuerySpec {
            query_id,
            channel: channel.clone(),
            req_type: AscPullReqType::account_info_by_hash(next),
            account: Account::ZERO,
            hash: next,
            cooldown_account: false,
        })
    }

    fn count_queries_by_hash(&self, hash: &BlockHash, source: QuerySource) -> usize {
        self.running_queries
            .iter_hash(hash)
            .filter(|i| i.source == source)
            .count()
    }

    /* Waits for next available blocking block */
    pub fn next_blocking(&self) -> BlockHash {
        self.candidate_accounts
            .next_blocking(|hash| self.count_queries_by_hash(hash, QuerySource::Dependencies) == 0)
    }

    pub(crate) fn process_response(
        &mut self,
        response: AscPullAck,
        channel_id: ChannelId,
        now: Timestamp,
    ) -> Result<ProcessInfo, ProcessError> {
        let query = self.take_running_query_for(&response)?;
        self.scoring.received_message(channel_id);
        self.process_response_for_query(&query, response)
            .map(|_| ProcessInfo::new(&query, now))
    }

    fn take_running_query_for(
        &mut self,
        response: &AscPullAck,
    ) -> Result<RunningQuery, ProcessError> {
        // Only process messages that have a known running query
        let Some(query) = self.running_queries.remove(response.id) else {
            return Err(ProcessError::NoRunningQueryFound);
        };

        if !query.is_valid_response_type(response) {
            return Err(ProcessError::InvalidResponseType);
        }

        Ok(query)
    }

    fn process_response_for_query(
        &mut self,
        query: &RunningQuery,
        response: AscPullAck,
    ) -> Result<(), ProcessError> {
        let ok = match response.pull_type {
            AscPullAckType::Blocks(blocks) => {
                self.response_blocks += 1;
                self.block_ack_processor
                    .process(&mut self.candidate_accounts, query, blocks)
            }
            AscPullAckType::AccountInfo(info) => {
                self.response_account += 1;
                self.account_ack_processor
                    .process(&mut self.candidate_accounts, query, &info)
            }
            AscPullAckType::Frontiers(frontiers) => {
                self.response_frontiers += 1;
                self.frontiers_processor.process(query, frontiers)
            }
        };

        if ok {
            Ok(())
        } else {
            Err(ProcessError::InvalidResponse)
        }
    }

    pub fn frontiers_processed(&mut self, outdated: &OutdatedAccounts) {
        self.frontiers_processor
            .frontiers_processed(outdated, &mut self.candidate_accounts);
    }

    pub fn container_info(&self) -> ContainerInfo {
        ContainerInfo::builder()
            .leaf(
                "tags",
                self.running_queries.len(),
                RunningQueryContainer::ELEMENT_SIZE,
            )
            .node("accounts", self.candidate_accounts.container_info())
            .node("frontiers", self.frontiers_processor.container_info())
            .node("peers", self.scoring.container_info())
            .finish()
    }
}

impl Default for BootstrapLogic {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl StatsSource for BootstrapLogic {
    fn collect_stats(&self, result: &mut StatsCollection) {
        const BOOTSTRAP_PROCESS: &'static str = "bootstrap_process";
        result.insert(BOOTSTRAP_PROCESS, "blocks", self.response_blocks);
        result.insert(BOOTSTRAP_PROCESS, "account_info", self.response_account);
        result.insert(BOOTSTRAP_PROCESS, "frontiers", self.response_frontiers);

        self.frontiers_processor.collect_stats(result);
        self.account_ack_processor.collect_stats(result);
        self.block_ack_processor.collect_stats(result);
    }
}

#[derive(Debug)]
pub(crate) enum ProcessError {
    NoRunningQueryFound,
    InvalidResponseType,
    InvalidResponse,
}

pub(crate) struct ProcessInfo {
    pub query_type: QueryType,
    pub response_time: Duration,
}

impl ProcessInfo {
    pub fn new(query: &RunningQuery, now: Timestamp) -> Self {
        Self {
            query_type: query.query_type,
            response_time: query.sent.elapsed(now),
        }
    }
}
