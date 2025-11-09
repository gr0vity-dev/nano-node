use std::sync::{Arc, Mutex, RwLock};

use bounded_vec_deque::BoundedVecDeque;

use rsnano_ledger::Ledger;
use rsnano_messages::NetworkFilter;
use rsnano_network::{Network, PeerConnector, TcpListener, TcpListenerExt};
use rsnano_network_protocol::InboundMessageQueue;
use rsnano_nullable_clock::SteadyClock;
use rsnano_utils::stats::Stats;
use tracing::warn;

use crate::{
    block_processing::{
        BlockProcessor, BlockProcessorQueue, BoundedBacklog, LocalBlockBroadcaster,
        LocalBlockBroadcasterExt,
    },
    block_rate_calculator::CurrentBlockRates,
    bootstrap::{BootstrapExt, BootstrapServer, Bootstrapper},
    cementation::ConfirmingSet,
    config::{NodeConfig, NodeFlags},
    consensus::{
        ActiveElectionsContainer, CurrentRepTiers, LocalVoteHistory, RequestAggregator, VoteCache,
        VoteCacheProcessor, VoteGenerators, VoteProcessor, VoteProcessorExt, VoteProcessorQueue,
        VoteRebroadcaster, WinnerBlockBroadcaster, election::ConfirmedElection,
        election_schedulers::ElectionSchedulers,
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

#[cfg(feature = "ledger_snapshots")]
use crate::ledger_snapshots::LedgerSnapshots;

use rsnano_types::PrivateKey;
use rsnano_wallet::Wallets;

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

#[derive(Clone)]
pub struct NetworkServices {
    pub network: Arc<RwLock<Network>>,
    pub tcp_listener: Arc<TcpListener>,
    pub peer_connector: Arc<PeerConnector>,
    pub(crate) network_threads: Arc<Mutex<NetworkThreads>>,
    pub message_processor: Arc<Mutex<MessageProcessor>>,
    pub message_sender: Arc<Mutex<MessageSender>>,
    pub message_flooder: Arc<Mutex<MessageFlooder>>,
    pub keepalive_publisher: Arc<KeepalivePublisher>,
    pub inbound_message_queue: Arc<InboundMessageQueue>,
    pub network_filter: Arc<NetworkFilter>,
    pub steady_clock: Arc<SteadyClock>,
}

impl NetworkServices {
    pub(crate) fn new(
        network: Arc<RwLock<Network>>,
        tcp_listener: Arc<TcpListener>,
        peer_connector: Arc<PeerConnector>,
        network_threads: Arc<Mutex<NetworkThreads>>,
        message_processor: Arc<Mutex<MessageProcessor>>,
        message_sender: Arc<Mutex<MessageSender>>,
        message_flooder: Arc<Mutex<MessageFlooder>>,
        keepalive_publisher: Arc<KeepalivePublisher>,
        inbound_message_queue: Arc<InboundMessageQueue>,
        network_filter: Arc<NetworkFilter>,
        steady_clock: Arc<SteadyClock>,
    ) -> Self {
        Self {
            network,
            tcp_listener,
            peer_connector,
            network_threads,
            message_processor,
            message_sender,
            message_flooder,
            keepalive_publisher,
            inbound_message_queue,
            network_filter,
            steady_clock,
        }
    }

    pub fn start(&self, max_inbound_connections: usize) {
        self.network_threads.lock().unwrap().start();
        if max_inbound_connections > 0 {
            self.tcp_listener.start();
        } else {
            warn!("Peering is disabled");
        }
        self.message_processor.lock().unwrap().start();
    }

    pub fn stop(&self) {
        self.stop_listeners();
        self.stop_threads();
    }

    pub fn stop_listeners(&self) {
        self.tcp_listener.stop();
        self.peer_connector.stop();
    }

    pub fn stop_threads(&self) {
        self.message_processor.lock().unwrap().stop();
        self.network_threads.lock().unwrap().stop();
    }
}

/// Bundles the core `Arc` collaborators that make up a running node so tests and
/// higher layers can grab a focused subset without touching the gigantic
/// `Node` struct directly.
#[derive(Clone)]
pub struct NodeServices {
    pub steady_clock: Arc<SteadyClock>,
    pub stats: Arc<Stats>,
    pub work_factory: Arc<WorkFactory>,
    pub ledger: Arc<Ledger>,
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

    pub fn network_services(&self) -> NetworkServices {
        NetworkServices::new(
            self.network.clone(),
            self.tcp_listener.clone(),
            self.peer_connector.clone(),
            self.network_threads.clone(),
            self.message_processor.clone(),
            self.message_sender.clone(),
            self.message_flooder.clone(),
            self.keepalive_publisher.clone(),
            self.inbound_message_queue.clone(),
            self.network_filter.clone(),
            self.steady_clock.clone(),
        )
    }

    pub fn consensus_services(&self) -> ConsensusServices {
        ConsensusServices::new(
            self.active.clone(),
            self.election_schedulers.clone(),
            self.vote_processor.clone(),
            self.vote_generators.clone(),
            self.vote_history.clone(),
            self.request_aggregator.clone(),
            self.bounded_backlog.clone(),
            self.bootstrapper.clone(),
            self.rep_crawler.clone(),
            self.online_reps.clone(),
            self.rep_tiers.clone(),
            self.local_block_broadcaster.clone(),
            self.winner_block_broadcaster.clone(),
            self.vote_processor_queue.clone(),
            self.vote_cache.clone(),
            self.vote_cache_processor.clone(),
            self.confirming_set.clone(),
            self.block_processor.clone(),
            self.block_processor_queue.clone(),
            self.vote_rebroadcaster.clone(),
        )
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
    pub ledger: Arc<Ledger>,
    pub block_rates: Arc<CurrentBlockRates>,
    pub confirming_set: Arc<ConfirmingSet>,
    pub recently_cemented: Arc<Mutex<BoundedVecDeque<ConfirmedElection>>>,
    pub stats: Arc<Stats>,
}

impl LedgerQueryServices {
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
