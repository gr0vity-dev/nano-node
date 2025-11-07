use std::sync::{Arc, Mutex, RwLock};

use bounded_vec_deque::BoundedVecDeque;

use rsnano_ledger::Ledger;
use rsnano_messages::NetworkFilter;
use rsnano_network::{Network, PeerConnector, TcpListener};
use rsnano_network_protocol::InboundMessageQueue;
use rsnano_nullable_clock::SteadyClock;
use rsnano_utils::stats::Stats;

use crate::{
    block_processing::{
        BlockProcessor, BlockProcessorQueue, BoundedBacklog, LocalBlockBroadcaster,
    },
    block_rate_calculator::CurrentBlockRates,
    bootstrap::{BootstrapServer, Bootstrapper},
    cementation::ConfirmingSet,
    consensus::{
        ActiveElectionsContainer, CurrentRepTiers, LocalVoteHistory, RequestAggregator, VoteCache,
        VoteGenerators, VoteProcessor, VoteProcessorQueue, WinnerBlockBroadcaster,
        election::ConfirmedElection, election_schedulers::ElectionSchedulers,
    },
    representatives::{OnlineReps, RepCrawler},
    telemetry::Telemetry,
    transport::{MessageFlooder, MessageSender, NetworkThreads, keepalive::KeepalivePublisher},
    wallets::WalletRepresentatives,
    work::WorkFactory,
};

#[cfg(feature = "ledger_snapshots")]
use crate::ledger_snapshots::LedgerSnapshots;

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
}

#[derive(Clone)]
pub struct NetworkServices {
    pub network: Arc<RwLock<Network>>,
    pub tcp_listener: Arc<TcpListener>,
    pub peer_connector: Arc<PeerConnector>,
    pub(crate) network_threads: Arc<Mutex<NetworkThreads>>,
    pub message_sender: Arc<Mutex<MessageSender>>,
    pub message_flooder: Arc<Mutex<MessageFlooder>>,
    pub keepalive_publisher: Arc<KeepalivePublisher>,
    pub inbound_message_queue: Arc<InboundMessageQueue>,
    pub network_filter: Arc<NetworkFilter>,
}

impl NetworkServices {
    pub(crate) fn new(
        network: Arc<RwLock<Network>>,
        tcp_listener: Arc<TcpListener>,
        peer_connector: Arc<PeerConnector>,
        network_threads: Arc<Mutex<NetworkThreads>>,
        message_sender: Arc<Mutex<MessageSender>>,
        message_flooder: Arc<Mutex<MessageFlooder>>,
        keepalive_publisher: Arc<KeepalivePublisher>,
        inbound_message_queue: Arc<InboundMessageQueue>,
        network_filter: Arc<NetworkFilter>,
    ) -> Self {
        Self {
            network,
            tcp_listener,
            peer_connector,
            network_threads,
            message_sender,
            message_flooder,
            keepalive_publisher,
            inbound_message_queue,
            network_filter,
        }
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
    pub message_sender: Arc<Mutex<MessageSender>>,
    pub message_flooder: Arc<Mutex<MessageFlooder>>,
    pub keepalive_publisher: Arc<KeepalivePublisher>,
    pub recently_cemented: Arc<Mutex<BoundedVecDeque<ConfirmedElection>>>,
    pub block_rates: Arc<CurrentBlockRates>,
    pub wallet_reps: Arc<Mutex<WalletRepresentatives>>,
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
            self.message_sender.clone(),
            self.message_flooder.clone(),
            self.keepalive_publisher.clone(),
            self.inbound_message_queue.clone(),
            self.network_filter.clone(),
        )
    }
}
