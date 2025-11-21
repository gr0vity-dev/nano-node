use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use rsnano_node::{
    block_processing::BlockSource,
    cementation::ConfirmingSetInfo,
    consensus::{ActiveElectionsInfo, RepTier},
};
use rsnano_nullable_clock::{SteadyClock, Timestamp};
use rsnano_types::{Account, BlockHash};
use rsnano_utils::fair_queue::FairQueueInfo;

use crate::{
    bootstrap::BootstrapInfo,
    channels::Channels,
    explorer::Explorer,
    frontier_scan::FrontierScanInfo,
    ledger_stats::LedgerStats,
    message_collection::MessageCollection,
    message_recorder::MessageRecorder,
    navigator::{NavItem, Navigator},
    node_callbacks::NodeCallbackFactory,
    node_runner::NodeRunner,
};

pub(crate) struct InsightApp {
    last_update: Option<Timestamp>,
    pub clock: Arc<SteadyClock>,
    pub messages: Arc<RwLock<MessageCollection>>,
    pub msg_recorder: Arc<MessageRecorder>,
    pub node_runner: NodeRunner,
    pub channels: Channels,
    pub explorer: Explorer,
    pub navigator: Navigator,
    pub ledger_stats: LedgerStats,
    pub aec_info: ActiveElectionsInfo,
    pub max_hinted: usize,
    pub max_optimistic: usize,
    pub confirming_set: ConfirmingSetInfo,
    pub block_processor_info: FairQueueInfo<BlockSource>,
    pub vote_processor_info: FairQueueInfo<RepTier>,
    pub frontier_scan: FrontierScanInfo,
    pub bootstrap: BootstrapInfo,
    pub rollback_hash: String,
}

impl InsightApp {
    pub fn new() -> Self {
        let clock = Arc::new(SteadyClock::default());
        let messages = Arc::new(RwLock::new(MessageCollection::default()));
        let msg_recorder = Arc::new(MessageRecorder::new(messages.clone()));
        let callback_factory = NodeCallbackFactory::new(msg_recorder.clone(), clock.clone());
        let channels = Channels::new(messages.clone());
        Self {
            clock,
            messages,
            msg_recorder,
            node_runner: NodeRunner::new(callback_factory),
            channels,
            explorer: Explorer::new(),
            navigator: Navigator::new(),
            ledger_stats: LedgerStats::new(),
            aec_info: Default::default(),
            max_hinted: 1,
            max_optimistic: 1,
            confirming_set: Default::default(),
            block_processor_info: Default::default(),
            vote_processor_info: Default::default(),
            frontier_scan: FrontierScanInfo::default(),
            last_update: None,
            bootstrap: Default::default(),
            rollback_hash: String::new(),
        }
    }

    pub fn search(&mut self, input: &str) {
        if let Some(node) = self.node_runner.node() {
            let has_result = self
                .explorer
                .search(&node.ledger_query_services().ledger_arc(), input);
            if has_result {
                self.navigator.current = NavItem::Explorer;
            }
        }
    }

    pub(crate) fn update(&mut self) -> bool {
        let now = self.clock.now();
        if let Some(last_update) = self.last_update
            && now - last_update < Duration::from_millis(500)
        {
            return false;
        }

        if let Some(node) = self.node_runner.node() {
            self.ledger_stats.update(&node);
            let channels = node.network_subsystem().sorted_channels();
            let telemetries = node.telemetry_subsystem().telemetry().get_all_telemetries();
            let (peered_reps, min_rep_weight) = {
                let consensus_services = node.consensus_subsystem();
                let guard = consensus_services.online_reps.lock().unwrap();
                (guard.peered_reps(), guard.minimum_principal_weight())
            };
            self.channels
                .update(channels, telemetries, peered_reps, min_rep_weight);
            self.aec_info = node.consensus_subsystem().active.read().unwrap().info();
            self.max_optimistic = node
                .consensus_subsystem()
                .election_schedulers
                .optimistic
                .max_elections;
            self.max_hinted = node
                .consensus_subsystem()
                .election_schedulers
                .hinted
                .max_elections;
            self.confirming_set = node.consensus_subsystem().confirming_set.info();
            self.block_processor_info = node.consensus_subsystem().block_processor_queue.info();
            self.vote_processor_info = node.consensus_subsystem().vote_processor_queue.info();
            {
                let bootstrap_services = node.bootstrap_work_services();
                let state = bootstrap_services.bootstrapper.state();
                self.frontier_scan.update(&state, now);
                self.bootstrap.update(&state);
            }
        }

        self.last_update = Some(now);
        true
    }

    pub(crate) fn add_priority_account(&mut self) {
        if let Some(account) = Account::parse(&self.bootstrap.add_account) {
            self.bootstrap.add_account.clear();
            if let Some(node) = self.node_runner.node() {
                let bootstrap_services = node.bootstrap_work_services();
                bootstrap_services
                    .bootstrapper
                    .state()
                    .candidate_accounts
                    .priority_up(&account);
            }
        }
    }

    pub(crate) fn roll_back(&self) {
        if let Some(hash) = BlockHash::decode_hex(&self.rollback_hash)
            && let Some(node) = self.node_runner.node()
        {
            let _ = node
                .ledger_query_services()
                .ledger_arc()
                .roll_back(&hash);
        }
    }
}
