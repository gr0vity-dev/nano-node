use std::{
    ops::{Deref, DerefMut},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use bounded_vec_deque::BoundedVecDeque;

use crate::{
    block_processing::{
        BacklogScan, BlockProcessor, BlockProcessorQueue, BoundedBacklog, LocalBlockBroadcaster,
        LocalBlockBroadcasterExt,
    },
    block_rate_calculator::CurrentBlockRates,
    bootstrap::{BootstrapExt, BootstrapServer, Bootstrapper},
    cementation::ConfirmingSet,
    config::{NetworkParams, NodeConfig, NodeFlags},
    consensus::{
        ActiveElectionsContainer, AecTicker, AecVoter, CurrentRepTiers, LocalVoteHistory,
        RequestAggregator, VoteCache, VoteCacheProcessor, VoteGenerators, VoteProcessor,
        VoteProcessorExt, VoteProcessorQueue, VoteRebroadcaster, WinnerBlockBroadcaster,
        election::ConfirmedElection, election_schedulers::ElectionSchedulers,
    },
    representatives::{OnlineReps, RepCrawler, RepCrawlerExt},
    telemetry::{TelementryExt, Telemetry},
    transport::{
        MessageFlooder, MessageProcessor, MessageSender, NetworkThreads,
        keepalive::KeepalivePublisher,
    },
    wallets::WalletRepresentatives,
    work::WorkFactory,
};
use rsnano_ledger::Ledger;
use rsnano_messages::NetworkFilter;
use rsnano_network::{Network, PeerConnector, TcpListener};
use rsnano_network_protocol::InboundMessageQueue;
use rsnano_nullable_clock::SteadyClock;
use rsnano_utils::{
    stats::Stats,
    ticker::{TickerPool, TimerThread},
};

#[cfg(feature = "ledger_snapshots")]
use crate::ledger_snapshots::LedgerSnapshots;

use rsnano_types::PrivateKey;
use rsnano_wallet::Wallets;

pub struct ConsensusTimerServices<'a> {
    aec_ticker: &'a TimerThread<AecTicker>,
    aec_voter: &'a TimerThread<AecVoter>,
}

impl<'a> ConsensusTimerServices<'a> {
    pub(crate) fn new(
        aec_ticker: &'a TimerThread<AecTicker>,
        aec_voter: &'a TimerThread<AecVoter>,
    ) -> Self {
        Self {
            aec_ticker,
            aec_voter,
        }
    }

    pub fn start(&self, flags: &NodeFlags, network_params: &NetworkParams) {
        self.aec_voter.start(Duration::from_millis(20));
        if !flags.disable_request_loop {
            self.aec_ticker
                .start(network_params.network.aec_loop_interval);
        }
    }

    pub fn stop(&self) {
        self.aec_ticker.stop();
        self.aec_voter.stop();
    }

    #[cfg(test)]
    pub fn ticker(&self) -> &TimerThread<AecTicker> {
        self.aec_ticker
    }
}

pub struct TickerServices {
    ticker_pool: TickerPool,
}

impl TickerServices {
    pub(crate) fn new(ticker_pool: TickerPool) -> Self {
        Self { ticker_pool }
    }

    pub fn start(&mut self) {
        self.ticker_pool.start();
    }

    pub fn stop(&mut self) {
        self.ticker_pool.stop();
    }

    pub fn ticker_pool(&self) -> &TickerPool {
        &self.ticker_pool
    }
}

#[derive(Clone)]
pub struct WalletServices {
    pub wallets: Arc<Wallets>,
    pub work_factory: Arc<WorkFactory>,
    pub wallet_reps: Arc<Mutex<WalletRepresentatives>>,
}

impl WalletServices {
    pub(crate) fn new(
        wallets: Arc<Wallets>,
        work_factory: Arc<WorkFactory>,
        wallet_reps: Arc<Mutex<WalletRepresentatives>>,
    ) -> Self {
        Self {
            wallets,
            work_factory,
            wallet_reps,
        }
    }

    pub fn insert_into_wallet(&self, keys: &PrivateKey) {
        let wallet_id = self.wallets.wallet_ids()[0];
        self.wallets
            .insert_adhoc2(&wallet_id, &keys.raw_key(), true)
            .unwrap();
    }

    pub fn stop(&self) {
        self.wallets.stop();
    }
}

pub struct BacklogServices {
    backlog_scan: BacklogScan,
}

impl BacklogServices {
    pub(crate) fn new(backlog_scan: BacklogScan) -> Self {
        Self { backlog_scan }
    }

    pub fn start(&mut self) {
        self.backlog_scan.start();
    }

    pub fn stop(&mut self) {
        self.backlog_scan.stop();
    }
}

impl Deref for BacklogServices {
    type Target = BacklogScan;

    fn deref(&self) -> &Self::Target {
        &self.backlog_scan
    }
}

impl DerefMut for BacklogServices {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.backlog_scan
    }
}

#[derive(Clone)]
pub struct TelemetryServices {
    pub telemetry: Arc<Telemetry>,
    pub tcp_listener: Arc<TcpListener>,
}

impl TelemetryServices {
    pub(crate) fn new(telemetry: Arc<Telemetry>, tcp_listener: Arc<TcpListener>) -> Self {
        Self {
            telemetry,
            tcp_listener,
        }
    }

    pub fn start(&self) {
        self.telemetry.start();
    }

    pub fn stop(&self) {
        self.telemetry.stop();
    }
}

/// Bundles the core `Arc` collaborators that make up a running node so tests and
/// higher layers can grab a focused subset without touching the gigantic
/// `Node` struct directly.
#[derive(Clone)]
pub(crate) struct NodeServices {
    pub steady_clock: Arc<SteadyClock>,
    pub stats: Arc<Stats>,
    pub work_factory: Arc<WorkFactory>,
    ledger: Arc<Ledger>,
    pub network: Arc<RwLock<Network>>,
    pub telemetry: Arc<Telemetry>,
    pub bootstrap_server: Arc<BootstrapServer>,
    pub online_reps: Arc<Mutex<OnlineReps>>,
    pub rep_tiers: Arc<CurrentRepTiers>,
    pub vote_processor_queue: Arc<VoteProcessorQueue>,
    pub vote_history: Arc<LocalVoteHistory>,
    pub confirming_set: Arc<ConfirmingSet>,
    pub vote_cache: Arc<Mutex<VoteCache>>,
    pub(crate) vote_cache_processor: Arc<VoteCacheProcessor>,
    pub block_processor: Arc<BlockProcessor>,
    pub block_processor_queue: Arc<BlockProcessorQueue>,
    pub wallets: Arc<Wallets>,
    pub vote_generators: Arc<VoteGenerators>,
    pub active: Arc<RwLock<ActiveElectionsContainer>>,
    pub vote_processor: Arc<VoteProcessor>,
    pub rep_crawler: Arc<RepCrawler>,
    pub tcp_listener: Arc<TcpListener>,
    pub election_schedulers: Arc<ElectionSchedulers>,
    pub request_aggregator: Arc<RequestAggregator>,
    pub bounded_backlog: Arc<BoundedBacklog>,
    pub bootstrapper: Arc<Bootstrapper>,
    pub local_block_broadcaster: Arc<LocalBlockBroadcaster>,
    pub(crate) network_threads: Arc<Mutex<NetworkThreads>>,
    pub peer_connector: Arc<PeerConnector>,
    pub inbound_message_queue: Arc<InboundMessageQueue>,
    pub network_filter: Arc<NetworkFilter>,
    pub message_processor: Arc<Mutex<MessageProcessor>>,
    pub message_sender: Arc<Mutex<MessageSender>>,
    pub message_flooder: Arc<Mutex<MessageFlooder>>,
    pub keepalive_publisher: Arc<KeepalivePublisher>,
    pub recently_cemented: Arc<Mutex<BoundedVecDeque<ConfirmedElection>>>,
    pub block_rates: Arc<CurrentBlockRates>,
    pub wallet_reps: Arc<Mutex<WalletRepresentatives>>,
    pub(crate) vote_rebroadcaster: Arc<Mutex<VoteRebroadcaster>>,
    pub(crate) winner_block_broadcaster: Arc<Mutex<WinnerBlockBroadcaster>>,
    #[cfg(feature = "ledger_snapshots")]
    pub ledger_snapshots: Arc<LedgerSnapshots>,
}

impl NodeServices {
    pub(crate) fn ledger(&self) -> Arc<Ledger> {
        self.ledger.clone()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        steady_clock: Arc<SteadyClock>,
        stats: Arc<Stats>,
        work_factory: Arc<WorkFactory>,
        ledger: Arc<Ledger>,
        network: Arc<RwLock<Network>>,
        telemetry: Arc<Telemetry>,
        bootstrap_server: Arc<BootstrapServer>,
        online_reps: Arc<Mutex<OnlineReps>>,
        rep_tiers: Arc<CurrentRepTiers>,
        vote_processor_queue: Arc<VoteProcessorQueue>,
        vote_history: Arc<LocalVoteHistory>,
        confirming_set: Arc<ConfirmingSet>,
        vote_cache: Arc<Mutex<VoteCache>>,
        vote_cache_processor: Arc<VoteCacheProcessor>,
        block_processor: Arc<BlockProcessor>,
        block_processor_queue: Arc<BlockProcessorQueue>,
        wallets: Arc<Wallets>,
        vote_generators: Arc<VoteGenerators>,
        active: Arc<RwLock<ActiveElectionsContainer>>,
        vote_processor: Arc<VoteProcessor>,
        rep_crawler: Arc<RepCrawler>,
        tcp_listener: Arc<TcpListener>,
        election_schedulers: Arc<ElectionSchedulers>,
        request_aggregator: Arc<RequestAggregator>,
        bounded_backlog: Arc<BoundedBacklog>,
        bootstrapper: Arc<Bootstrapper>,
        local_block_broadcaster: Arc<LocalBlockBroadcaster>,
        network_threads: Arc<Mutex<NetworkThreads>>,
        peer_connector: Arc<PeerConnector>,
        inbound_message_queue: Arc<InboundMessageQueue>,
        network_filter: Arc<NetworkFilter>,
        message_processor: Arc<Mutex<MessageProcessor>>,
        message_sender: Arc<Mutex<MessageSender>>,
        message_flooder: Arc<Mutex<MessageFlooder>>,
        keepalive_publisher: Arc<KeepalivePublisher>,
        recently_cemented: Arc<Mutex<BoundedVecDeque<ConfirmedElection>>>,
        block_rates: Arc<CurrentBlockRates>,
        wallet_reps: Arc<Mutex<WalletRepresentatives>>,
        vote_rebroadcaster: Arc<Mutex<VoteRebroadcaster>>,
        winner_block_broadcaster: Arc<Mutex<WinnerBlockBroadcaster>>,
        #[cfg(feature = "ledger_snapshots")] ledger_snapshots: Arc<LedgerSnapshots>,
    ) -> Self {
        Self {
            steady_clock,
            stats,
            work_factory,
            ledger,
            network,
            telemetry,
            bootstrap_server,
            online_reps,
            rep_tiers,
            vote_processor_queue,
            vote_history,
            confirming_set,
            vote_cache,
            vote_cache_processor,
            block_processor,
            block_processor_queue,
            wallets,
            vote_generators,
            active,
            vote_processor,
            rep_crawler,
            tcp_listener,
            election_schedulers,
            request_aggregator,
            bounded_backlog,
            bootstrapper,
            local_block_broadcaster,
            network_threads,
            peer_connector,
            inbound_message_queue,
            network_filter,
            message_processor,
            message_sender,
            message_flooder,
            keepalive_publisher,
            recently_cemented,
            block_rates,
            wallet_reps,
            vote_rebroadcaster,
            winner_block_broadcaster,
            #[cfg(feature = "ledger_snapshots")]
            ledger_snapshots,
        }
    }

    pub fn wallet_services(&self) -> WalletServices {
        WalletServices::new(
            self.wallets.clone(),
            self.work_factory.clone(),
            self.wallet_reps.clone(),
        )
    }

    pub fn telemetry_services(&self) -> TelemetryServices {
        TelemetryServices::new(self.telemetry.clone(), self.tcp_listener.clone())
    }

    pub fn ledger_query_services(&self) -> LedgerQueryServices {
        LedgerQueryServices::new(
            self.ledger.clone(),
            self.block_rates.clone(),
            self.confirming_set.clone(),
            self.recently_cemented.clone(),
            self.stats.clone(),
        )
    }

    pub fn bootstrap_work_services(&self) -> BootstrapWorkServices {
        BootstrapWorkServices::new(
            self.bootstrapper.clone(),
            self.bootstrap_server.clone(),
            self.work_factory.clone(),
        )
    }
}
#[derive(Clone)]
pub struct ConsensusServices {
    pub active: Arc<RwLock<ActiveElectionsContainer>>,
    pub election_schedulers: Arc<ElectionSchedulers>,
    pub vote_processor: Arc<VoteProcessor>,
    pub vote_generators: Arc<VoteGenerators>,
    pub vote_history: Arc<LocalVoteHistory>,
    pub request_aggregator: Arc<RequestAggregator>,
    pub bounded_backlog: Arc<BoundedBacklog>,
    pub bootstrapper: Arc<Bootstrapper>,
    pub rep_crawler: Arc<RepCrawler>,
    pub online_reps: Arc<Mutex<OnlineReps>>,
    pub rep_tiers: Arc<CurrentRepTiers>,
    pub local_block_broadcaster: Arc<LocalBlockBroadcaster>,
    pub(crate) winner_block_broadcaster: Arc<Mutex<WinnerBlockBroadcaster>>,
    pub vote_processor_queue: Arc<VoteProcessorQueue>,
    pub vote_cache: Arc<Mutex<VoteCache>>,
    pub(crate) vote_cache_processor: Arc<VoteCacheProcessor>,
    pub confirming_set: Arc<ConfirmingSet>,
    pub block_processor: Arc<BlockProcessor>,
    pub block_processor_queue: Arc<BlockProcessorQueue>,
    pub(crate) vote_rebroadcaster: Arc<Mutex<VoteRebroadcaster>>,
}

impl ConsensusServices {
    pub(crate) fn new(
        active: Arc<RwLock<ActiveElectionsContainer>>,
        election_schedulers: Arc<ElectionSchedulers>,
        vote_processor: Arc<VoteProcessor>,
        vote_generators: Arc<VoteGenerators>,
        vote_history: Arc<LocalVoteHistory>,
        request_aggregator: Arc<RequestAggregator>,
        bounded_backlog: Arc<BoundedBacklog>,
        bootstrapper: Arc<Bootstrapper>,
        rep_crawler: Arc<RepCrawler>,
        online_reps: Arc<Mutex<OnlineReps>>,
        rep_tiers: Arc<CurrentRepTiers>,
        local_block_broadcaster: Arc<LocalBlockBroadcaster>,
        winner_block_broadcaster: Arc<Mutex<WinnerBlockBroadcaster>>,
        vote_processor_queue: Arc<VoteProcessorQueue>,
        vote_cache: Arc<Mutex<VoteCache>>,
        vote_cache_processor: Arc<VoteCacheProcessor>,
        confirming_set: Arc<ConfirmingSet>,
        block_processor: Arc<BlockProcessor>,
        block_processor_queue: Arc<BlockProcessorQueue>,
        vote_rebroadcaster: Arc<Mutex<VoteRebroadcaster>>,
    ) -> Self {
        Self {
            active,
            election_schedulers,
            vote_processor,
            vote_generators,
            vote_history,
            request_aggregator,
            bounded_backlog,
            bootstrapper,
            rep_crawler,
            online_reps,
            rep_tiers,
            local_block_broadcaster,
            winner_block_broadcaster,
            vote_processor_queue,
            vote_cache,
            vote_cache_processor,
            confirming_set,
            block_processor,
            block_processor_queue,
            vote_rebroadcaster,
        }
    }

    pub fn start(&self, config: &NodeConfig, flags: &NodeFlags) {
        if config.enable_vote_processor {
            self.vote_processor.start();
        }
        self.block_processor.start(config.block_processor_threads);
        if !flags.disable_rep_crawler {
            self.rep_crawler.start();
        }
        self.vote_generators.start();
        self.request_aggregator.start();
        self.confirming_set.start();
        self.election_schedulers.start();
        if config.enable_bounded_backlog {
            self.bounded_backlog.start();
        }
        self.local_block_broadcaster.start();
        self.vote_cache_processor.start();
        if config.enable_vote_rebroadcast {
            self.vote_rebroadcaster.lock().unwrap().start();
        }
    }

    pub fn stop(&self) {
        self.local_block_broadcaster.stop();
        self.request_aggregator.stop();
        self.vote_processor.stop();
        self.election_schedulers.stop();
        self.active.write().unwrap().stop();
        self.vote_generators.stop();
        self.confirming_set.stop();
        self.bounded_backlog.stop();
        self.rep_crawler.stop();
        self.block_processor.stop();
        self.vote_rebroadcaster.lock().unwrap().stop();
        self.vote_cache_processor.stop();
    }
}

#[derive(Clone)]
pub struct LedgerQueryServices {
    pub(crate) ledger: Arc<Ledger>,
    pub block_rates: Arc<CurrentBlockRates>,
    pub confirming_set: Arc<ConfirmingSet>,
    pub recently_cemented: Arc<Mutex<BoundedVecDeque<ConfirmedElection>>>,
    pub stats: Arc<Stats>,
}

impl LedgerQueryServices {
    /// Temporary escape hatch for components that still require deep ledger access.
    #[doc(hidden)]
    #[deprecated(
        note = "For tests/internal wiring only. Production code must use ProductionHandles / narrow APIs."
    )]
    pub fn ledger_arc(&self) -> Arc<Ledger> {
        self.ledger.clone()
    }

    pub(crate) fn new(
        ledger: Arc<Ledger>,
        block_rates: Arc<CurrentBlockRates>,
        confirming_set: Arc<ConfirmingSet>,
        recently_cemented: Arc<Mutex<BoundedVecDeque<ConfirmedElection>>>,
        stats: Arc<Stats>,
    ) -> Self {
        Self {
            ledger,
            block_rates,
            confirming_set,
            recently_cemented,
            stats,
        }
    }
}

#[derive(Clone)]
pub struct BootstrapWorkServices {
    pub bootstrapper: Arc<Bootstrapper>,
    pub bootstrap_server: Arc<BootstrapServer>,
    pub work_factory: Arc<WorkFactory>,
}

impl BootstrapWorkServices {
    pub(crate) fn new(
        bootstrapper: Arc<Bootstrapper>,
        bootstrap_server: Arc<BootstrapServer>,
        work_factory: Arc<WorkFactory>,
    ) -> Self {
        Self {
            bootstrapper,
            bootstrap_server,
            work_factory,
        }
    }

    pub fn start(&self, enable_bootstrap_responder: bool) {
        if enable_bootstrap_responder {
            self.bootstrap_server.start();
        }
        self.bootstrapper.start();
    }

    pub fn stop(&self) {
        self.bootstrapper.stop();
        self.bootstrap_server.stop();
        self.work_factory.stop();
    }
}
