use std::{
    sync::{Arc, Condvar, Mutex, MutexGuard, RwLock},
    thread::JoinHandle,
    time::Duration,
};

use tracing::{trace, warn};

use rsnano_ledger::{Ledger, ProcessResult};
use rsnano_messages::{AscPullAck, BlocksAckPayload};
use rsnano_network::{ChannelId, DeadChannelCleanupStep, Network, token_bucket::TokenBucket};
use rsnano_nullable_clock::SteadyClock;
use rsnano_types::Account;
use rsnano_utils::container_info::{ContainerInfo, ContainerInfoProvider};
use rsnano_utils::stats::{DetailType, Sample, StatType, Stats, StatsCollection, StatsSource};

use super::{
    FrontierScanConfig,
    block_inspector::BlockInspector,
    cleanup::BootstrapCleanup,
    requesters::Requesters,
    response_processor::ResponseProcessor,
    state::{BootstrapLogic, CandidateAccountsConfig},
};
use crate::{
    block_processing::BlockProcessorQueue,
    bootstrap::state::{PriorityUpResult, bootstrap_logic::ProcessError},
    transport::MessageSender,
};

#[derive(Clone, Debug, PartialEq)]
pub struct BootstrapConfig {
    pub enable: bool,
    pub enable_priorities: bool,
    pub enable_dependency_walker: bool,
    pub enable_frontier_scan: bool,
    /// Maximum number of un-responded requests per channel, should be lower or equal to bootstrap server max queue size
    pub channel_limit: usize,
    /// Limit of outgoing messages/s over all channels
    pub rate_limit: usize,
    pub database_rate_limit: usize,
    pub frontier_rate_limit: usize,
    pub database_warmup_ratio: usize,
    pub max_pull_count: u8,
    pub request_timeout: Duration,
    pub throttle_coefficient: usize,
    pub throttle_wait: Duration,
    pub block_processor_theshold: usize,
    /** Minimum accepted protocol version used when bootstrapping */
    pub min_protocol_version: u8,
    pub max_requests: usize,
    pub optimistic_request_percentage: u8,
    pub candidate_accounts: CandidateAccountsConfig,
    pub frontier_scan: FrontierScanConfig,
    /// How many frontier acks can get queued in the processor
    pub max_pending_frontier_responses: usize,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            enable: false,
            enable_priorities: false,
            enable_dependency_walker: false,
            enable_frontier_scan: false,
            channel_limit: 16,
            rate_limit: 500,
            database_rate_limit: 256,
            frontier_rate_limit: 8,
            database_warmup_ratio: 10,
            max_pull_count: BlocksAckPayload::MAX_BLOCKS,
            request_timeout: Duration::from_secs(15),
            throttle_coefficient: 8 * 1024,
            throttle_wait: Duration::from_millis(100),
            block_processor_theshold: 1000,
            min_protocol_version: 0x14, // TODO don't hard code
            max_requests: 1024,
            optimistic_request_percentage: 75,
            candidate_accounts: Default::default(),
            frontier_scan: Default::default(),
            max_pending_frontier_responses: 16,
        }
    }
}

pub struct Bootstrapper {
    stats: Arc<Stats>,
    threads: Mutex<Option<Threads>>,
    logic: Arc<Mutex<BootstrapLogic>>,
    state_changed: Arc<Condvar>,
    config: BootstrapConfig,
    clock: Arc<SteadyClock>,
    response_handler: ResponseProcessor,
    block_inspector: BlockInspector,
    requesters: Requesters,
}

struct Threads {
    cleanup: JoinHandle<()>,
}

impl Bootstrapper {
    pub(crate) fn new(
        block_processor_queue: Arc<BlockProcessorQueue>,
        ledger: Arc<Ledger>,
        stats: Arc<Stats>,
        network: Arc<RwLock<Network>>,
        message_sender: MessageSender,
        config: BootstrapConfig,
        clock: Arc<SteadyClock>,
    ) -> Self {
        let limiter = Arc::new(Mutex::new(TokenBucket::new(config.rate_limit)));
        let state = Arc::new(Mutex::new(BootstrapLogic::new(config.clone())));
        let state_changed = Arc::new(Condvar::new());

        let mut response_handler = ResponseProcessor::new(
            state.clone(),
            stats.clone(),
            block_processor_queue.clone(),
            ledger.clone(),
        );
        response_handler.set_max_pending_frontiers(config.max_pending_frontier_responses);

        let block_inspector = BlockInspector::new(
            state.clone(),
            ledger.clone(),
            stats.clone(),
            clock.clone(),
            block_processor_queue.clone(),
        );

        let requesters = Requesters::new(
            limiter.clone(),
            config.clone(),
            stats.clone(),
            message_sender.clone(),
            state.clone(),
            state_changed.clone(),
            clock.clone(),
            ledger.clone(),
            block_processor_queue,
            network,
        );

        Self {
            threads: Mutex::new(None),
            logic: state,
            state_changed,
            config,
            stats,
            clock,
            response_handler,
            block_inspector,
            requesters,
        }
    }

    pub fn new_null() -> Self {
        let block_processor_queue = Arc::new(BlockProcessorQueue::default());
        let ledger = Arc::new(Ledger::new_null());
        let stats = Arc::new(Stats::default());
        let network = Arc::new(RwLock::new(Network::new_test_instance()));
        let message_sender = MessageSender::new_null();
        let config = BootstrapConfig::default();
        let clock = Arc::new(SteadyClock::new_null());

        Self::new(
            block_processor_queue,
            ledger,
            stats,
            network,
            message_sender,
            config,
            clock,
        )
    }

    pub fn initialize(&self, genesis_account: &Account) {
        let inserted = self
            .logic
            .lock()
            .unwrap()
            .candidate_accounts
            .priority_up(genesis_account);

        if inserted == PriorityUpResult::Inserted {
            self.priority_inserted()
        } else {
            self.priority_insertion_failed()
        };
    }

    pub fn stop(&self) {
        {
            let mut guard = self.logic.lock().unwrap();
            guard.stopped = true;
        }
        self.state_changed.notify_all();

        self.requesters.stop();

        let threads = self.threads.lock().unwrap().take();
        if let Some(threads) = threads {
            threads.cleanup.join().unwrap();
        }
    }

    pub fn state(&self) -> MutexGuard<'_, BootstrapLogic> {
        self.logic.lock().unwrap()
    }

    pub fn prioritized(&self, account: &Account) -> bool {
        self.logic
            .lock()
            .unwrap()
            .candidate_accounts
            .prioritized(account)
    }

    fn run_timeouts(&self) {
        let mut cleanup = BootstrapCleanup::new(self.clock.clone(), self.stats.clone());
        let mut state = self.logic.lock().unwrap();
        while !state.stopped {
            cleanup.cleanup(&mut state);
            self.state_changed.notify_all();

            state = self
                .state_changed
                .wait_timeout_while(state, Duration::from_secs(1), |s| !s.stopped)
                .unwrap()
                .0;
        }
    }

    /// Process `asc_pull_ack` message coming from network
    pub fn process(&self, message: AscPullAck, channel_id: ChannelId) {
        let now = self.clock.now();
        let query_id = message.id;
        let result = self.response_handler.process(message, channel_id, now);
        match result {
            Ok(info) => {
                trace!(query_id, ?channel_id, "Response processed");
                self.stats.inc(StatType::Bootstrap, DetailType::Reply);
                self.stats
                    .inc(StatType::BootstrapReply, info.query_type.into());
                self.stats.sample(
                    Sample::BootstrapTagDuration,
                    info.response_time.as_millis() as i64,
                    (0, self.config.request_timeout.as_millis() as i64),
                );
            }
            Err(error) => {
                trace!(query_id, ?channel_id, ?error, "Response processing failed");
                match error {
                    ProcessError::NoRunningQueryFound => {
                        self.stats.inc(StatType::Bootstrap, DetailType::MissingTag);
                    }
                    ProcessError::InvalidResponseType => {
                        self.stats
                            .inc(StatType::Bootstrap, DetailType::InvalidResponseType);
                    }
                    ProcessError::InvalidResponse => {
                        self.stats
                            .inc(StatType::Bootstrap, DetailType::InvalidResponse);
                    }
                }
            }
        }
    }

    fn priority_inserted(&self) {
        self.stats
            .inc(StatType::BootstrapAccountSets, DetailType::PriorityInsert);
    }

    fn priority_insertion_failed(&self) {
        self.stats
            .inc(StatType::BootstrapAccountSets, DetailType::PrioritizeFailed);
    }

    pub fn inspect_blocks(&self, batch: &[ProcessResult]) {
        self.block_inspector.inspect(batch);
        self.state_changed.notify_all();
    }

    pub fn unblock_batch(&self, accounts: impl IntoIterator<Item = Account>) {
        let mut guard = self.logic.lock().unwrap();
        for account in accounts {
            guard.candidate_accounts.unblock(account, None);
        }
    }
}

impl Drop for Bootstrapper {
    fn drop(&mut self) {
        // All threads must be stopped before destruction
        debug_assert!(self.threads.lock().unwrap().is_none());
    }
}

impl ContainerInfoProvider for Bootstrapper {
    fn container_info(&self) -> ContainerInfo {
        self.logic.lock().unwrap().container_info()
    }
}

impl StatsSource for Bootstrapper {
    fn collect_stats(&self, result: &mut StatsCollection) {
        self.logic.lock().unwrap().collect_stats(result);
        self.requesters.collect_stats(result);
    }
}

pub trait BootstrapExt {
    fn start(&self);
}

impl BootstrapExt for Arc<Bootstrapper> {
    fn start(&self) {
        debug_assert!(self.threads.lock().unwrap().is_none());

        if !self.config.enable {
            warn!("Ascending bootstrap is disabled");
            return;
        }

        self.requesters.start();

        let self_l = Arc::clone(self);
        let cleanup = std::thread::Builder::new()
            .name("Bootstrap clean".to_string())
            .spawn(Box::new(move || self_l.run_timeouts()))
            .unwrap();

        *self.threads.lock().unwrap() = Some(Threads { cleanup });
    }
}

pub(crate) struct BootstrapperCleanup(pub Arc<Bootstrapper>);

impl DeadChannelCleanupStep for BootstrapperCleanup {
    fn clean_up_dead_channels(&self, dead_channel_ids: &[ChannelId]) {
        self.0
            .logic
            .lock()
            .unwrap()
            .scoring
            .clean_up_dead_channels(dead_channel_ids);
    }
}
