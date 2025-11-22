use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock,
        mpsc::{self, SyncSender},
    },
    time::Duration,
};

use anyhow::{Context, anyhow};
use bounded_vec_deque::BoundedVecDeque;
use tracing::{info, warn};

use rsnano_ledger::{Ledger, RepWeightCache};
use rsnano_messages::{Message, NetworkFilter};
use rsnano_network::{
    ChannelId, DeadChannelCleanup, Network, NetworkCleanup, PeerConnector, TcpListener,
    TcpNetworkAdapter, TrafficType,
};
use rsnano_network_protocol::{
    InboundMessageQueue, InboundMessageQueueCleanup, LatestKeepalives, LatestKeepalivesCleanup,
    MessageCallback, SynCookies,
};
use rsnano_nullable_clock::{SteadyClock, SystemTimeFactory};
use rsnano_store_lmdb::EnvironmentFlags;
use rsnano_types::{KeyDerivationFunction, Networks, NodeId, PrivateKey};
use rsnano_utils::{
    CancellationToken,
    container_info::ContainerInfoFactory,
    get_cpu_count,
    stats::{Stats, StatsCollector},
    sync::backpressure_channel::{self, Sender},
    thread_pool::ThreadPool,
    ticker::{Tickable, TickerPool, TimerThread},
};
use rsnano_wallet::{ReceivableSearch, WalletBackup, Wallets, WalletsTicker};

#[cfg(feature = "ledger_snapshots")]
use crate::ledger_snapshots::{LedgerSnapshots, fork_detector::ForkDetector};
use crate::{
    BacklogServices, BootstrapWorkServices, LedgerQueryServices, Node, NodeArgs, NodeEvent,
    OnlineWeightSampler, TickerServices, WalletServices,
    aec_event_processor::AecEventProcessor,
    block_processing::{
        BacklogScan, BacklogWaiter, BlockProcessor, BlockProcessorQueue, BoundedBacklog,
        BoundedBacklogPlugin, LedgerEvent, LocalBlockBroadcaster, LocalBlockBroadcasterPlugin,
        ProcessQueueConfig, UncheckedBlockReenqueuer, UncheckedMap,
    },
    block_rate_calculator::{BlockRateCalculator, CurrentBlockRates},
    bootstrap::{BootstrapResponderCleanup, BootstrapServer, Bootstrapper, BootstrapperCleanup},
    cementation::{ConfirmingSet, TrackConfirmationTimes},
    composition::{
        FoundationBits, NetworkIoBits, TelemetryBits, build_foundation, build_network_io,
        build_telemetry_bits,
    },
    config::{
        DaemonConfig, DaemonToml, GlobalConfig, NetworkParams, NodeConfig, NodeFlags,
        get_node_toml_config_path,
    },
    consensus::{
        ActiveElectionsContainer, AecEvent, AecForkInserter, AecTicker, AecVoter,
        BootstrapElectionActivator, BootstrapStaleElections, ConfirmReqSender,
        ConfirmationSolicitorPlugin, CpsLimiter, CurrentRepTiers, DependentElectionsConfirmer,
        ForkCache, ForkCacheUpdater, LocalVoteHistory, LocalVotesRemover, RepTiersCalculator,
        RequestAggregator, RequestAggregatorCleanup, VoteApplier, VoteBroadcaster, VoteCache,
        VoteCacheProcessor, VoteGenerators, VoteProcessor, VoteProcessorQueue,
        VoteProcessorQueueCleanup, VoteRebroadcastQueue, VoteRebroadcaster, WalletRepsChecker,
        WinnerBlockBroadcaster,
        election::ConfirmedElection,
        election_schedulers::{ElectionSchedulers, ElectionSchedulersPlugin},
    },
    ledger_event_processor::{LedgerEventProcessor, LedgerEventProcessorPlugin},
    node_id_key_file::NodeIdKeyFile,
    node_monitor::NodeMonitor,
    recently_cemented_inserter::RecentlyCementedInserter,
    representatives::{OnlineReps, OnlineRepsCleanup, OnlineWeightCalculation, RepCrawler},
    telemetry::Telemetry,
    tokio_runner::TokioRunner,
    transport::{
        MessageFlooder, MessageProcessor, MessageSender, NetworkMessageProcessor, NetworkThreads,
        PeerCacheConnector, PeerCacheUpdater, keepalive::{KeepaliveMessageFactory, KeepalivePublisher},
        run_loopback_channel_adapter,
    },
    utils::spawn_backpressure_processor,
    wallets::{
        LocalRepsComputation, WalletRepresentatives, block_processor::WalletBlockProcessor,
        work::WalletWorkProvider,
    },
    work::WorkFactory,
    working_path_for,
};
use store_traits::{
    config::LedgerBackend, environment::StoreEnvironmentOptions, types::StoreEnvironmentFlags,
    wallet_environment_factory::WalletEnvironmentFactory,
};

#[derive(Default)]
pub struct NodeCallbacks {
    pub on_publish: Option<MessageCallback>,
    pub on_inbound: Option<MessageCallback>,
    pub on_inbound_dropped: Option<MessageCallback>,
}

impl NodeCallbacks {
    pub fn builder() -> NodeCallbacksBuilder {
        NodeCallbacksBuilder::new()
    }
}

pub struct NodeCallbacksBuilder(NodeCallbacks);

impl NodeCallbacksBuilder {
    fn new() -> Self {
        Self(NodeCallbacks::default())
    }

    pub fn on_publish(
        mut self,
        callback: impl Fn(ChannelId, &Message) + Send + Sync + 'static,
    ) -> Self {
        self.0.on_publish = Some(Arc::new(callback));
        self
    }

    pub fn on_inbound(
        mut self,
        callback: impl Fn(ChannelId, &Message) + Send + Sync + 'static,
    ) -> Self {
        self.0.on_inbound = Some(Arc::new(callback));
        self
    }

    pub fn on_inbound_dropped(
        mut self,
        callback: impl Fn(ChannelId, &Message) + Send + Sync + 'static,
    ) -> Self {
        self.0.on_inbound_dropped = Some(Arc::new(callback));
        self
    }

    pub fn finish(self) -> NodeCallbacks {
        self.0
    }
}

pub struct NodeBuilder {
    network: Networks,
    data_path: Option<PathBuf>,
    config: Option<NodeConfig>,
    network_params: Option<NetworkParams>,
    flags: Option<NodeFlags>,
    callbacks: Option<NodeCallbacks>,
    event_sink: Option<SyncSender<NodeEvent>>,
    storage_backend: Option<LedgerBackend>,
}

pub(crate) struct ComposedNode {
    pub(crate) is_nulled: bool,
    pub(crate) runtime: tokio::runtime::Handle,
    pub(crate) data_path: PathBuf,
    pub(crate) node_id: PrivateKey,
    pub(crate) config: NodeConfig,
    pub(crate) network_params: NetworkParams,
    pub(crate) workers: Arc<ThreadPool>,
    pub(crate) flags: NodeFlags,
    // Wiring fields
    pub(crate) network: Arc<RwLock<Network>>,
    pub(crate) tcp_listener: Arc<TcpListener>,
    pub(crate) peer_connector: Arc<PeerConnector>,
    pub(crate) network_threads: Arc<Mutex<NetworkThreads>>,
    pub(crate) message_processor: Arc<Mutex<MessageProcessor>>,
    pub(crate) message_sender: Arc<Mutex<MessageSender>>,
    pub(crate) message_flooder: Arc<Mutex<MessageFlooder>>,
    pub(crate) keepalive_publisher: Arc<KeepalivePublisher>,
    pub(crate) inbound_message_queue: Arc<InboundMessageQueue>,
    pub(crate) network_filter: Arc<NetworkFilter>,
    pub(crate) steady_clock: Arc<SteadyClock>,

    pub(crate) active: Arc<RwLock<ActiveElectionsContainer>>,
    pub(crate) election_schedulers: Arc<ElectionSchedulers>,
    pub(crate) vote_processor: Arc<VoteProcessor>,
    pub(crate) vote_generators: Arc<VoteGenerators>,
    pub(crate) vote_history: Arc<LocalVoteHistory>,
    pub(crate) request_aggregator: Arc<RequestAggregator>,
    pub(crate) bounded_backlog: Arc<BoundedBacklog>,
    pub(crate) bootstrapper: Arc<Bootstrapper>,
    pub(crate) rep_crawler: Arc<RepCrawler>,
    pub(crate) online_reps: Arc<Mutex<OnlineReps>>,
    pub(crate) rep_tiers: Arc<CurrentRepTiers>,
    pub(crate) local_block_broadcaster: Arc<LocalBlockBroadcaster>,
    pub(crate) winner_block_broadcaster: Arc<Mutex<WinnerBlockBroadcaster>>,
    pub(crate) vote_processor_queue: Arc<VoteProcessorQueue>,
    pub(crate) vote_cache: Arc<Mutex<VoteCache>>,
    pub(crate) vote_cache_processor: Arc<VoteCacheProcessor>,
    pub(crate) confirming_set: Arc<ConfirmingSet>,
    pub(crate) block_processor: Arc<BlockProcessor>,
    pub(crate) block_processor_queue: Arc<BlockProcessorQueue>,
    pub(crate) vote_rebroadcaster: Arc<Mutex<VoteRebroadcaster>>,

    pub(crate) telemetry: Arc<Telemetry>,
    pub(crate) bootstrap_server: Arc<BootstrapServer>,
    pub(crate) work_factory: Arc<WorkFactory>,
    pub(crate) stats: Arc<Stats>,
    pub(crate) ledger: Arc<Ledger>,
    pub(crate) wallet_services: WalletServices,
    pub(crate) ledger_query_services: LedgerQueryServices,
    pub(crate) bootstrap_work_services: BootstrapWorkServices,
    pub(crate) unchecked: Arc<Mutex<UncheckedMap>>,
    pub(crate) backlog_scan: BacklogServices,
    pub(crate) tokio_runner: TokioRunner,
    pub(crate) aec_ticker: Arc<TimerThread<AecTicker>>,
    pub(crate) stats_collector: StatsCollector,
    pub(crate) container_info_factory: ContainerInfoFactory,
    pub(crate) aec_voter: Arc<TimerThread<AecVoter>>,
    pub(crate) ticker_services: TickerServices,
    #[cfg(feature = "ledger_snapshots")]
    pub(crate) ledger_snapshots: Arc<LedgerSnapshots>,
}
impl NodeBuilder {
    pub fn new(network: Networks) -> Self {
        Self {
            network,
            data_path: None,
            config: None,
            network_params: None,
            flags: None,
            callbacks: None,
            event_sink: None,
            storage_backend: None,
        }
    }

    pub fn data_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.data_path = Some(path.into());
        self
    }

    pub fn config(mut self, config: NodeConfig) -> Self {
        self.config = Some(config);
        self
    }

    pub fn storage_backend(mut self, backend: LedgerBackend) -> Self {
        self.storage_backend = Some(backend);
        self
    }

    pub fn network_params(mut self, network_params: NetworkParams) -> Self {
        self.network_params = Some(network_params);
        self
    }

    pub fn flags(mut self, flags: NodeFlags) -> Self {
        self.flags = Some(flags);
        self
    }

    pub fn callbacks(mut self, callbacks: NodeCallbacks) -> Self {
        self.callbacks = Some(callbacks);
        self
    }

    pub fn event_sink(mut self, sender: SyncSender<NodeEvent>) -> Self {
        self.event_sink = Some(sender);
        self
    }

    pub fn get_data_path(&self) -> anyhow::Result<PathBuf> {
        match &self.data_path {
            Some(path) => Ok(path.clone()),
            None => working_path_for(self.network).ok_or_else(|| anyhow!("working path not found")),
        }
    }

    pub fn finish(self) -> anyhow::Result<Node> {
        let data_path = self.get_data_path()?;

        let network_params = self
            .network_params
            .unwrap_or_else(|| NetworkParams::new(self.network));

        let mut config = match self.config {
            Some(c) => c,
            None => {
                let cpu_count = get_cpu_count();
                let mut daemon_config = DaemonConfig::new(&network_params, cpu_count);
                let config_path = get_node_toml_config_path(&data_path);
                if config_path.exists() {
                    let toml_str = std::fs::read_to_string(config_path)?;
                    let daemon_toml: DaemonToml = toml::de::from_str(&toml_str)?;
                    daemon_config.merge_toml(&daemon_toml);
                }
                daemon_config.node
            }
        };

        if let Some(selection) = self.storage_backend {
            config.ledger_store_config.backend = selection;
        }

        let flags = self.flags.unwrap_or_default();
        let callbacks = self.callbacks.unwrap_or_default();

        let args = NodeArgs {
            data_path,
            config,
            network_params,
            flags,
            callbacks,
            event_sender: self.event_sink,
        };

        Node::new_with_args(args)
    }
}

struct InfrastructureBits {
    work_factory: Arc<WorkFactory>,
    wallets: Arc<Wallets>,
    wallet_reps: Arc<Mutex<WalletRepresentatives>>,
}

struct NetworkBits {
    inbound_message_queue: Arc<InboundMessageQueue>,
    network: Arc<RwLock<Network>>,
    network_filter: Arc<NetworkFilter>,
    unchecked: Arc<Mutex<UncheckedMap>>,
    online_reps: Arc<Mutex<OnlineReps>>,
    message_sender: MessageSender,
    message_flooder: MessageFlooder,
}

struct NetworkThreadBits {
    network_threads: Arc<Mutex<NetworkThreads>>,
    message_processor: Arc<Mutex<MessageProcessor>>,
}

struct ConsensusBits {
    vote_processor_queue: Arc<VoteProcessorQueue>,
    vote_history: Arc<LocalVoteHistory>,
    confirming_set: Arc<ConfirmingSet>,
    vote_cache: Arc<Mutex<VoteCache>>,
    fork_cache: Arc<RwLock<ForkCache>>,
    block_processor_queue: Arc<BlockProcessorQueue>,
    unchecked_reenqueuer: UncheckedBlockReenqueuer,
}

struct ConsensusRuntime {
    vote_generators: Arc<VoteGenerators>,
    active_elections: Arc<RwLock<ActiveElectionsContainer>>,
    block_rates: Arc<CurrentBlockRates>,
    cps_limiter: CpsLimiter,
    vote_processor: Arc<VoteProcessor>,
    vote_cache_processor: Arc<VoteCacheProcessor>,
    recently_cemented: Arc<Mutex<BoundedVecDeque<ConfirmedElection>>>,
    winner_block_broadcaster: Arc<Mutex<WinnerBlockBroadcaster>>,
    aec_sender: backpressure_channel::Sender<AecEvent>,
    aec_receiver: backpressure_channel::Receiver<AecEvent>,
}

struct IntegrationBits {
    backlog_scan: BacklogScan,
    bounded_backlog: Arc<BoundedBacklog>,
    request_aggregator: Arc<RequestAggregator>,
    ledger_event_plugins: Vec<Box<dyn LedgerEventProcessorPlugin>>,
}

fn build_infrastructure(
    runtime: &tokio::runtime::Handle,
    config: &NodeConfig,
    network_params: &NetworkParams,
    application_path: &PathBuf,
    is_nulled: bool,
    wallet_env_factory: Arc<dyn WalletEnvironmentFactory>,
    ledger: &Arc<Ledger>,
    steady_clock: &Arc<SteadyClock>,
    global_config: &GlobalConfig,
    block_processor_queue: &Arc<BlockProcessorQueue>,
    online_reps: &Arc<Mutex<OnlineReps>>,
) -> anyhow::Result<InfrastructureBits> {
    let work_factory = Arc::new(
        WorkFactory::builder(runtime.clone())
            .local_work_pool(|p| {
                p.threads(config.work_threads as usize)
                    .cpu_rate_limit(Duration::from_millis(config.pow_sleep_interval_ns as u64))
                    .opencl_config(config.opencl.clone())
                    .enable_gpu(config.enable_opencl)
            })
            .work_peers(config.work_peers.clone())
            .finish(),
    );
    info!(
        "Work pool threads: {} ({})",
        work_factory.work_threads(),
        if work_factory.has_opencl() {
            "OpenCL"
        } else {
            "CPU"
        }
    );
    info!("Work peers: {}", config.work_peers.len());

    let mut wallets_path = application_path.clone();
    wallets_path.push("wallets.ldb");

    let wallets_config = global_config.wallets_config();

    let env_flags = if is_nulled {
        EnvironmentFlags::empty()
    } else {
        EnvironmentFlags::NO_SUB_DIR | EnvironmentFlags::NO_TLS | EnvironmentFlags::NO_READAHEAD
    };
    let env_bundle = wallet_env_factory
        .create(
            StoreEnvironmentOptions {
                path: wallets_path,
                max_databases: 128,
                map_size: 1024 * 1024 * 1024,
                flags: StoreEnvironmentFlags::from_bits(env_flags.bits()),
            },
            wallets_config.password_fanout,
            KeyDerivationFunction::new(wallets_config.kdf_work),
        )
        .context("Failed to initialize wallet environment")?;

    let mut wallets = Wallets::new(
        wallets_config.clone(),
        env_bundle.environment,
        ledger.clone(),
        network_params.work.clone(),
        steady_clock.clone(),
        env_bundle.store_factory,
    );
    if !is_nulled {
        wallets
            .initialize()
            .context("Failed to initialize wallets database")?;
    }

    let wallets = Arc::new(wallets);

    let (tx_work, rx_work) = mpsc::channel();
    wallets.set_work_queue(tx_work);

    let (tx_block, rx_block) = mpsc::channel();
    wallets.set_block_queue(tx_block);

    let wallet_work = WalletWorkProvider::new(wallets.clone(), rx_work, work_factory.clone());

    std::thread::Builder::new()
        .name("Wallet work".to_owned())
        .spawn(move || wallet_work.run())
        .context("Failed to spawn wallet work thread")?;

    let wallet_blocks =
        WalletBlockProcessor::new(rx_block, wallets.clone(), block_processor_queue.clone());

    std::thread::Builder::new()
        .name("Wallet blocks".to_owned())
        .spawn(move || wallet_blocks.run())
        .context("Failed to spawn wallet blocks thread")?;

    let wallet_reps = Arc::new(Mutex::new(WalletRepresentatives::new(
        wallets_config.voting_enabled,
        wallets_config.vote_minimum,
        ledger.rep_weights.clone(),
        wallets.clone(),
        online_reps.clone(),
    )));
    wallet_reps.lock().unwrap().compute_reps();

    Ok(InfrastructureBits {
        work_factory,
        wallets,
        wallet_reps,
    })
}

fn build_network(
    config: &NodeConfig,
    network_params: &NetworkParams,
    node_id: NodeId,
    _node_id_key: &PrivateKey,
    current_network: Networks,
    runtime: &tokio::runtime::Handle,
    callbacks: &NodeCallbacks,
    steady_clock: &Arc<SteadyClock>,
    stats: &Arc<Stats>,
    ledger: &Arc<Ledger>,
    rep_weights: &Arc<RepWeightCache>,
    _flags: &NodeFlags,
    ticker_pool: &mut TickerPool,
) -> NetworkBits {
    let mut inbound_message_queue = InboundMessageQueue::new(config.message_processor.max_queue);
    if let Some(cb) = callbacks.on_inbound.clone() {
        inbound_message_queue.set_inbound_callback(cb);
    }
    if let Some(cb) = callbacks.on_inbound_dropped.clone() {
        inbound_message_queue.set_inbound_dropped_callback(cb);
    }
    let inbound_message_queue = Arc::new(inbound_message_queue);

    let network = Network::new(config.network.clone());
    runtime.spawn(run_loopback_channel_adapter(
        network.loopback().clone(),
        node_id,
        current_network,
        inbound_message_queue.clone(),
    ));
    let network = Arc::new(RwLock::new(network));

    let mut network_filter = NetworkFilter::new(config.network_duplicate_filter_size);
    network_filter.age_cutoff = config.network_duplicate_filter_cutoff;
    let network_filter = Arc::new(network_filter);

    let unchecked = Arc::new(Mutex::new(UncheckedMap::new(
        config.max_unchecked_blocks as usize,
    )));

    let online_reps = Arc::new(Mutex::new(
        OnlineReps::builder()
            .rep_weights(rep_weights.clone())
            .online_weight_minimum(config.online_weight_minimum)
            .representative_weight_minimum(config.representative_vote_weight_minimum)
            .weight_interval(OnlineReps::default_interval_for(current_network))
            .finish(),
    ));

    let online_weight_sampler =
        OnlineWeightSampler::new(ledger.clone(), network_params.network.current_network);

    let mut online_weight_calculation = OnlineWeightCalculation::new(
        online_weight_sampler,
        online_reps.clone(),
        steady_clock.clone(),
    );
    online_weight_calculation.tick(&CancellationToken::new());
    ticker_pool.insert(
        online_weight_calculation,
        OnlineReps::default_interval_for(current_network),
    );

    let mut message_sender =
        MessageSender::new(stats.clone(), network_params.network.protocol_info());

    if let Some(callback) = callbacks.on_publish.clone() {
        message_sender.set_published_callback(callback);
    }

    let message_flooder = MessageFlooder::new(
        online_reps.clone(),
        network.clone(),
        stats.clone(),
        message_sender.clone(),
    );

    NetworkBits {
        inbound_message_queue,
        network,
        network_filter,
        unchecked,
        online_reps,
        message_sender,
        message_flooder,
    }
}

fn build_network_threads(
    config: &NodeConfig,
    flags: &NodeFlags,
    network_params: &NetworkParams,
    steady_clock: &Arc<SteadyClock>,
    stats: &Arc<Stats>,
    network: &Arc<RwLock<Network>>,
    inbound_message_queue: &Arc<InboundMessageQueue>,
    network_filter: &Arc<NetworkFilter>,
    online_reps: &Arc<Mutex<OnlineReps>>,
    bootstrap_server: &Arc<BootstrapServer>,
    vote_processor_queue: &Arc<VoteProcessorQueue>,
    block_processor_queue: &Arc<BlockProcessorQueue>,
    latest_keepalives: &Arc<Mutex<LatestKeepalives>>,
    network_adapter: &Arc<TcpNetworkAdapter>,
    request_aggregator: &Arc<RequestAggregator>,
    bootstrapper: &Arc<Bootstrapper>,
    peer_connector: &Arc<PeerConnector>,
    syn_cookies: &Arc<SynCookies>,
    keepalive_factory: &Arc<KeepaliveMessageFactory>,
    message_flooder: &MessageFlooder,
    telemetry: &Arc<Telemetry>,
    wallet_reps: &Arc<Mutex<WalletRepresentatives>>,
    #[cfg(feature = "ledger_snapshots")] ledger_snapshots: &Arc<LedgerSnapshots>,
) -> NetworkThreadBits {
    let mut dead_channel_cleanup = DeadChannelCleanup::new(
        steady_clock.clone(),
        network.clone(),
        network_params.network.cleanup_cutoff(),
    );
    dead_channel_cleanup.add_step(InboundMessageQueueCleanup::new(
        inbound_message_queue.clone(),
    ));
    dead_channel_cleanup.add_step(OnlineRepsCleanup::new(online_reps.clone()));
    dead_channel_cleanup.add_step(BootstrapResponderCleanup::new(
        bootstrap_server.server_impl.clone(),
    ));
    dead_channel_cleanup.add_step(VoteProcessorQueueCleanup::new(vote_processor_queue.clone()));
    dead_channel_cleanup.add_step(block_processor_queue.clone());
    dead_channel_cleanup.add_step(LatestKeepalivesCleanup::new(latest_keepalives.clone()));
    dead_channel_cleanup.add_step(NetworkCleanup::new(network_adapter.clone()));
    dead_channel_cleanup.add_step(RequestAggregatorCleanup::new(
        request_aggregator.state.clone(),
    ));
    dead_channel_cleanup.add_step(BootstrapperCleanup(bootstrapper.clone()));

    let network_message_processor = Arc::new(NetworkMessageProcessor::new(
        stats.clone(),
        network.clone(),
        network_filter.clone(),
        block_processor_queue.clone(),
        wallet_reps.clone(),
        request_aggregator.clone(),
        vote_processor_queue.clone(),
        telemetry.clone(),
        bootstrap_server.clone(),
        bootstrapper.clone(),
        network_params.work.clone(),
        #[cfg(feature = "ledger_snapshots")]
        ledger_snapshots.clone(),
    ));

    let network_threads = Arc::new(Mutex::new(NetworkThreads::new(
        network.clone(),
        peer_connector.clone(),
        flags.clone(),
        network_params.clone(),
        config.network.clone(),
        stats.clone(),
        syn_cookies.clone(),
        network_filter.clone(),
        keepalive_factory.clone(),
        latest_keepalives.clone(),
        dead_channel_cleanup,
        message_flooder.clone(),
        steady_clock.clone(),
    )));

    let message_processor = Arc::new(Mutex::new(MessageProcessor::new(
        config.clone(),
        inbound_message_queue.clone(),
        network_message_processor.clone(),
    )));

    NetworkThreadBits {
        network_threads,
        message_processor,
    }
}

fn build_consensus_bits(
    config: &NodeConfig,
    global_config: &GlobalConfig,
    ledger: &Arc<Ledger>,
    stats: &Arc<Stats>,
    network_params: &NetworkParams,
    ledger_tx: &Sender<LedgerEvent>,
    unchecked: &Arc<Mutex<UncheckedMap>>,
    steady_clock: &Arc<SteadyClock>,
) -> ConsensusBits {
    let vote_processor_queue = Arc::new(VoteProcessorQueue::new(
        config.vote_processor.clone(),
        stats.clone(),
    ));

    let vote_history = Arc::new(LocalVoteHistory::new(
        network_params.network.current_network,
    ));

    let confirming_set = Arc::new(ConfirmingSet::new(
        config.confirming_set.clone(),
        ledger.clone(),
        stats.clone(),
    ));
    confirming_set.set_event_publisher(ledger_tx.clone());

    let vote_cache = Arc::new(Mutex::new(VoteCache::new(
        config.vote_cache.clone(),
        stats.clone(),
    )));

    let fork_cache = Arc::new(RwLock::new(ForkCache::with(
        config.fork_cache_max_size,
        config.fork_cache_max_forks_per_root,
    )));

    let block_processor_config = ProcessQueueConfig::from(global_config);
    let block_processor_queue = Arc::new(BlockProcessorQueue::new(block_processor_config));

    let unchecked_reenqueuer = UncheckedBlockReenqueuer::new(
        unchecked.clone(),
        ledger.clone(),
        block_processor_queue.clone(),
        steady_clock.clone(),
    );

    ConsensusBits {
        vote_processor_queue,
        vote_history,
        confirming_set,
        vote_cache,
        fork_cache,
        block_processor_queue,
        unchecked_reenqueuer,
    }
}

fn build_consensus_runtime(
    config: &NodeConfig,
    current_network: Networks,
    steady_clock: &Arc<SteadyClock>,
    ledger: &Arc<Ledger>,
    stats: &Arc<Stats>,
    network_params: &NetworkParams,
    wallet_reps: &Arc<Mutex<WalletRepresentatives>>,
    vote_history: &Arc<LocalVoteHistory>,
    vote_cache: &Arc<Mutex<VoteCache>>,
    vote_processor_queue: &Arc<VoteProcessorQueue>,
    message_flooder: &MessageFlooder,
    message_sender: &MessageSender,
    online_reps: &Arc<Mutex<OnlineReps>>,
    rep_weights: &Arc<RepWeightCache>,
    ticker_pool: &mut TickerPool,
    event_queues_info: &mut ContainerInfoFactory,
    network: &Arc<RwLock<Network>>,
) -> ConsensusRuntime {
    let vote_broadcaster = Arc::new(VoteBroadcaster::new(
        vote_processor_queue.clone(),
        message_flooder.clone(),
        stats.clone(),
    ));

    let vote_generators = Arc::new(VoteGenerators::new(
        ledger.clone(),
        wallet_reps.clone(),
        vote_history.clone(),
        stats.clone(),
        config,
        network_params,
        vote_broadcaster.clone(),
        message_sender.clone(),
        steady_clock.clone(),
    ));

    let base_latency = match current_network {
        Networks::NanoDevNetwork => Duration::from_millis(25),
        _ => Duration::from_millis(1000),
    };

    let (aec_sender, aec_receiver) = backpressure_channel::channel(1024 * 5);
    let aec_sender_clone = aec_sender.clone();
    event_queues_info.add_leaf("aec", move || aec_sender_clone.len());

    let mut active_elections =
        ActiveElectionsContainer::new(config.active_elections.clone(), base_latency);
    active_elections.set_observer(aec_sender.clone());
    let active_elections = Arc::new(RwLock::new(active_elections));

    let block_rate_calculator = BlockRateCalculator::new(steady_clock.clone(), ledger.clone());
    let block_rates = block_rate_calculator.rates().clone();
    ticker_pool.insert(block_rate_calculator, Duration::from_millis(500));
    let cps_limiter = if config.cps_limit > 0 {
        info!(
            "Confirmations per second (CPS) is limited to: {}",
            config.cps_limit
        );
        CpsLimiter::new(block_rates.clone(), config.cps_limit as usize)
    } else {
        info!("Unlimited confirmations per second (CPS)!");
        CpsLimiter::unlimited()
    };

    let vote_applier = VoteApplier::new(
        active_elections.clone(),
        online_reps.clone(),
        steady_clock.clone(),
        rep_weights.clone(),
        current_network == Networks::NanoDevNetwork,
    );

    let vote_processor = Arc::new(VoteProcessor::new(
        vote_processor_queue.clone(),
        vote_applier,
        stats.clone(),
    ));

    let vote_cache_processor = Arc::new(VoteCacheProcessor::new(
        stats.clone(),
        vote_cache.clone(),
        vote_processor_queue.clone(),
        config.vote_processor.clone(),
    ));

    let recently_cemented = Arc::new(Mutex::new(BoundedVecDeque::new(
        config.confirmation_history_size,
    )));

    let winner_block_broadcaster = Arc::new(Mutex::new(WinnerBlockBroadcaster::new(
        steady_clock.clone(),
        current_network,
        message_flooder.clone(),
        online_reps.clone(),
        network.clone(), // ??? need access
    )));

    ConsensusRuntime {
        vote_generators,
        active_elections,
        block_rates,
        cps_limiter,
        vote_processor,
        vote_cache_processor,
        recently_cemented,
        winner_block_broadcaster,
        aec_sender,
        aec_receiver,
    }
}

fn build_integration_bits(
    config: &mut NodeConfig,
    global_config: &GlobalConfig,
    ledger: &Arc<Ledger>,
    steady_clock: &Arc<SteadyClock>,
    stats: &Arc<Stats>,
    ledger_tx: &Sender<LedgerEvent>,
    vote_generators: &Arc<VoteGenerators>,
    election_schedulers: &Arc<ElectionSchedulers>,
) -> IntegrationBits {
    let mut backlog_scan =
        BacklogScan::new(global_config.into(), ledger.clone(), steady_clock.clone());

    // Hook backlog discoveries into the schedulers so accounts get activated promptly.
    let schedulers_w = Arc::downgrade(election_schedulers);
    let ledger_l = ledger.clone();
    backlog_scan.on_unconfirmed_found(move |batch| {
        if let Some(schedulers) = schedulers_w.upgrade() {
            let any = ledger_l.any();
            for info in batch {
                schedulers.activate_backlog(
                    &any,
                    &info.account,
                    &info.account_info,
                    &info.conf_info,
                );
            }
        }
    });

    let request_aggregator = Arc::new(RequestAggregator::new(
        config.request_aggregator.clone(),
        stats.clone(),
        vote_generators.clone(),
        ledger.clone(),
    ));

    if config.bounded_backlog.max_backlog == 0 {
        config.enable_bounded_backlog = false;
    }
    if !config.enable_bounded_backlog {
        config.bounded_backlog.max_backlog = 0;
    }

    let bounded_backlog = Arc::new(BoundedBacklog::new(
        config.bounded_backlog.clone(),
        ledger.clone(),
        stats.clone(),
        steady_clock.clone(),
        ledger_tx.clone(),
    ));

    let mut ledger_event_plugins: Vec<Box<dyn LedgerEventProcessorPlugin>> = Vec::new();

    if config.enable_bounded_backlog {
        info!(
            "Bounded backlog enabled: max backlog={}, batch_size={}, scan_rate={}",
            config.bounded_backlog.max_backlog,
            config.bounded_backlog.batch_size,
            config.bounded_backlog.scan_rate
        );

        ledger_event_plugins.push(Box::new(BoundedBacklogPlugin::new(bounded_backlog.clone())));

        backlog_scan.on_unconfirmed_found({
            let backlog_w = Arc::downgrade(&bounded_backlog);
            move |batch| {
                if let Some(backlog) = backlog_w.upgrade() {
                    backlog.activate_batch(batch);
                }
            }
        });

        backlog_scan.on_up_to_date({
            let backlog_w = Arc::downgrade(&bounded_backlog);
            move |batch| {
                if let Some(backlog) = backlog_w.upgrade() {
                    backlog.erase_accounts(batch);
                }
            }
        });
    }

    IntegrationBits {
        backlog_scan,
        bounded_backlog,
        request_aggregator,
        ledger_event_plugins,
    }
}

pub(crate) fn compose_root(
    args: NodeArgs,
    is_nulled: bool,
    mut node_id_key_file: NodeIdKeyFile,
) -> anyhow::Result<ComposedNode> {
    let NodeArgs {
        data_path,
        config,
        network_params,
        flags,
        callbacks,
        event_sender,
    } = args;

    let FoundationBits {
        tokio_runner,
        runtime,
        mut config,
        network_params,
        flags,
        application_path,
        steady_clock,
        stats,
        node_id_key,
        node_id,
        global_config,
        ledger,
        ledger_tx,
        ledger_rx,
        mut event_queues_info,
        workers,
        mut ticker_pool,
        current_network,
        wallet_env_factory,
        syn_cookies,
    } = build_foundation(
        config,
        network_params,
        flags,
        data_path,
        is_nulled,
        &mut node_id_key_file,
    )?;

    let node_observer = event_sender.clone();

    let mut ledger_event_processor_plugins: Vec<Box<dyn LedgerEventProcessorPlugin>> = Vec::new();
    let rep_weights = ledger.rep_weights.clone();

    let NetworkBits {
        inbound_message_queue,
        network,
        network_filter,
        unchecked,
        online_reps,
        message_sender,
        message_flooder,
    } = build_network(
        &config,
        &network_params,
        node_id,
        &node_id_key,
        current_network,
        &runtime,
        &callbacks,
        &steady_clock,
        &stats,
        &ledger,
        &rep_weights,
        &flags,
        &mut ticker_pool,
    );

    let ConsensusBits {
        vote_processor_queue,
        vote_history,
        confirming_set,
        vote_cache,
        fork_cache,
        block_processor_queue,
        unchecked_reenqueuer,
    } = build_consensus_bits(
        &config,
        &global_config,
        &ledger,
        &stats,
        &network_params,
        &ledger_tx,
        &unchecked,
        &steady_clock,
    );
    ticker_pool.insert(unchecked_reenqueuer.clone(), Duration::from_secs(1));

    let InfrastructureBits {
        work_factory,
        wallets,
        wallet_reps,
    } = build_infrastructure(
        &runtime,
        &config,
        &network_params,
        &application_path,
        is_nulled,
        Arc::clone(&wallet_env_factory),
        &ledger,
        &steady_clock,
        &global_config,
        &block_processor_queue,
        &online_reps,
    )?;

    let ConsensusRuntime {
        vote_generators,
        active_elections,
        block_rates,
        cps_limiter,
        vote_processor,
        vote_cache_processor,
        recently_cemented,
        winner_block_broadcaster,
        aec_sender,
        aec_receiver,
    } = build_consensus_runtime(
        &config,
        current_network,
        &steady_clock,
        &ledger,
        &stats,
        &network_params,
        &wallet_reps,
        &vote_history,
        &vote_cache,
        &vote_processor_queue,
        &message_flooder,
        &message_sender,
        &online_reps,
        &rep_weights,
        &mut ticker_pool,
        &mut event_queues_info,
        &network,
    );

    let confirm_req_sender = ConfirmReqSender::new(stats.clone(), steady_clock.clone());

    let election_schedulers = Arc::new(ElectionSchedulers::new(
        config.clone(),
        network_params.network.clone(),
        active_elections.clone(),
        ledger.clone(),
        stats.clone(),
        vote_cache.clone(),
        confirming_set.clone(),
        online_reps.clone(),
        steady_clock.clone(),
    ));
    ledger_event_processor_plugins.push(Box::new(ElectionSchedulersPlugin::new(
        election_schedulers.clone(),
    )));

    let IntegrationBits {
        backlog_scan,
        bounded_backlog,
        request_aggregator,
        mut ledger_event_plugins,
    } = build_integration_bits(
        &mut config,
        &global_config,
        &ledger,
        &steady_clock,
        &stats,
        &ledger_tx,
        &vote_generators,
        &election_schedulers,
    );
    ledger_event_processor_plugins.append(&mut ledger_event_plugins);

    let mut bootstrap_sender = MessageSender::new_with_buffer_size(
        stats.clone(),
        network_params.network.protocol_info(),
        512,
    );

    if let Some(callback) = &callbacks.on_publish {
        bootstrap_sender.set_published_callback(callback.clone());
    }

    let NetworkIoBits {
        network_adapter,
        peer_connector,
        keepalive_factory,
        keepalive_publisher,
        rep_crawler,
        tcp_listener,
    } = build_network_io(
        &config,
        &network_params,
        &runtime,
        &steady_clock,
        &stats,
        &network,
        &ledger,
        &message_sender,
        &online_reps,
        &active_elections,
    );

    let TelemetryBits {
        telemetry,
        bootstrap_server,
        data_receiver_factory,
        latest_keepalives,
        handshake_stats,
    } = build_telemetry_bits(
        &config,
        &flags,
        &network_params,
        &network,
        &inbound_message_queue,
        &network_filter,
        &stats,
        &syn_cookies,
        &node_id_key,
        &message_sender,
        &steady_clock,
        &ledger,
        &unchecked,
    );

    network
        .write()
        .unwrap()
        .set_data_receiver_factory(data_receiver_factory);
    // former (if TCP is active, otherwise `network` picks first)

    let track_conf_times = Box::new(TrackConfirmationTimes::default());
    let conf_time_stats = track_conf_times.stats();
    ledger_event_processor_plugins.push(track_conf_times);

    let bootstrapper = Arc::new(Bootstrapper::new(
        block_processor_queue.clone(),
        ledger.clone(),
        stats.clone(),
        network.clone(),
        message_sender.clone(),
        global_config.node_config.bootstrap.clone(),
        steady_clock.clone(),
    ));
    bootstrapper.initialize(&network_params.ledger.genesis_account);

    let mut aec_ticker = AecTicker::new(active_elections.clone(), steady_clock.clone());

    aec_ticker.add_plugin(ConfirmationSolicitorPlugin {
        message_flooder: message_flooder.clone(),
        online_reps: online_reps.clone(),
        winner_block_broadcaster: winner_block_broadcaster.clone(),
        confirm_req_sender,
    });

    let mut bootstrap_stale =
        BootstrapStaleElections::new(bootstrapper.clone(), steady_clock.clone());
    bootstrap_stale.set_stale_threshold(config.bootstrap_stale_threshold);
    let bootstrap_stale_stats = bootstrap_stale.stats.clone();
    aec_ticker.add_plugin(bootstrap_stale);

    let local_block_broadcaster = Arc::new(LocalBlockBroadcaster::new(
        config.local_block_broadcaster.clone(),
        stats.clone(),
        ledger.clone(),
        confirming_set.clone(),
        steady_clock.clone(),
        message_flooder.clone(),
        !flags.disable_block_processor_republishing,
    ));

    ledger_event_processor_plugins.push(Box::new(LocalBlockBroadcasterPlugin::new(
        local_block_broadcaster.clone(),
    )));

    let vote_cache_w = Arc::downgrade(&vote_cache);
    let active_w = Arc::downgrade(&active_elections);
    let scheduler_w = Arc::downgrade(&election_schedulers);
    let confirming_set_w = Arc::downgrade(&confirming_set);
    let local_block_broadcaster_w = Arc::downgrade(&local_block_broadcaster);

    // TODO: remove the duplication of the on_rolling_back event
    bounded_backlog.can_roll_back(move |hash| {
        if let Some(i) = vote_cache_w.upgrade() {
            if i.lock().unwrap().contains(hash) {
                return false;
            }
        }

        if let Some(i) = active_w.upgrade() {
            let guard = i.read().unwrap();
            if guard.is_active_hash(hash) || guard.was_recently_confirmed(hash) {
                return false;
            }
        }

        if let Some(i) = scheduler_w.upgrade() {
            if i.contains(hash) {
                return false;
            }
        }

        if let Some(i) = confirming_set_w.upgrade() {
            if i.contains(hash) {
                return false;
            }
        }

        if let Some(i) = local_block_broadcaster_w.upgrade() {
            if i.contains(hash) {
                return false;
            }
        }
        true
    });

    let backlog_waiter = Arc::new(BacklogWaiter::new(
        block_processor_queue.clone(),
        ledger.clone(),
        steady_clock.clone(),
        config.bounded_backlog.max_backlog,
    ));

    let ledger_tx_clone = ledger_tx.clone();
    let block_processor = Arc::new(BlockProcessor::new(
        block_processor_queue.clone(),
        ledger.clone(),
        unchecked.clone(),
        unchecked_reenqueuer.clone(),
        backlog_waiter.clone(),
        ledger_tx_clone,
        steady_clock.clone(),
    ));

    #[cfg(feature = "ledger_snapshots")]
    let ledger_snapshots = {
        let wallet_reps2 = wallet_reps.clone();
        Arc::new(LedgerSnapshots::new(
            ledger.clone(),
            move || {
                // TODO: make this nice:
                let mut keys = Vec::new();
                wallet_reps2.lock().unwrap().rep_priv_keys(&mut keys);
                // For simplicity only take the first key.
                // TODO: allow multiple keys
                keys.pop()
            },
            message_flooder.clone(),
            online_reps.clone(),
        ))
    };

    let NetworkThreadBits {
        network_threads,
        message_processor,
    } = build_network_threads(
        &config,
        &flags,
        &network_params,
        &steady_clock,
        &stats,
        &network,
        &inbound_message_queue,
        &network_filter,
        &online_reps,
        &bootstrap_server,
        &vote_processor_queue,
        &block_processor_queue,
        &latest_keepalives,
        &network_adapter,
        &request_aggregator,
        &bootstrapper,
        &peer_connector,
        &syn_cookies,
        &keepalive_factory,
        &message_flooder,
        &telemetry,
        &wallet_reps,
        #[cfg(feature = "ledger_snapshots")]
        &ledger_snapshots,
    );

    let rep_crawler_w: std::sync::Weak<RepCrawler> = Arc::downgrade(&rep_crawler);
    if !flags.disable_rep_crawler {
        network
            .write()
            .unwrap()
            .on_new_realtime_channel(Arc::new(move |channel| {
                if let Some(crawler) = rep_crawler_w.upgrade() {
                    crawler.query_with_priority(channel);
                }
            }));
    }

    let vote_rebroadcast_queue = Arc::new(
        VoteRebroadcastQueue::build()
            .max_len(config.vote_rebroadcaster_max_queue)
            .stats(stats.clone())
            .finish(),
    );

    let vote_rebroadcaster = Arc::new(Mutex::new(VoteRebroadcaster::new(
        vote_rebroadcast_queue.clone(),
        message_flooder.clone(),
        rep_weights.clone(),
        steady_clock.clone(),
        config.rebroadcast_history.clone(),
    )));

    let keepalive_factory_w: std::sync::Weak<KeepaliveMessageFactory> =
        Arc::downgrade(&keepalive_factory);
    let message_publisher_l = Arc::new(Mutex::new(message_sender.clone()));
    let message_publisher_w = Arc::downgrade(&message_publisher_l);
    network
        .write()
        .unwrap()
        .on_new_realtime_channel(Arc::new(move |channel| {
            // Send a keepalive message to the new channel
            let Some(factory) = keepalive_factory_w.upgrade() else {
                return;
            };
            let Some(publisher) = message_publisher_w.upgrade() else {
                return;
            };
            let keepalive = factory.create_keepalive_self();
            publisher
                .lock()
                .unwrap()
                .try_send(&channel, &keepalive, TrafficType::Keepalive);
        }));

    if !work_factory.work_generation_enabled() {
        info!("Work generation is disabled");
    }

    info!(
        "Outbound bandwidth limit: {} bytes/s, burst ratio: {}",
        config.network.limiter.generic_limit, config.network.limiter.generic_burst_ratio
    );

    let has_local_reps = {
        let reps = wallet_reps.lock().unwrap();
        let has_local_reps = reps.voting_reps() > 0;
        if has_local_reps {
            info!(
                "Found {} local representatives in wallets",
                reps.voting_reps()
            );
            for rep in reps.rep_accounts() {
                info!("Local representative: {}", rep.encode_account());
            }
        }

        has_local_reps
    };

    if has_local_reps {
        if config.enable_voting {
            let voting_reps = wallet_reps.lock().unwrap().voting_reps();
            info!(
                "Voting is enabled, more system resources will be used, local representatives: {voting_reps}"
            );
            if voting_reps > 1 {
                warn!("Voting with more than one representative can limit performance");
            }
        } else {
            warn!(
                "Found local representatives in wallets, but voting is disabled. To enable voting, set `[node] enable_voting=true`n the `config-node.toml` file or use `--enable_voting` command line argument"
            );
        }
    }

    let is_dev_network = network_params.network.is_dev_network();
    let time_factory = SystemTimeFactory::default();

    let peer_cache_updater = PeerCacheUpdater::new(
        network.clone(),
        ledger.clone(),
        time_factory,
        stats.clone(),
        if network_params.network.is_dev_network() {
            Duration::from_secs(10)
        } else {
            Duration::from_secs(60 * 60)
        },
    );
    ticker_pool.insert(
        peer_cache_updater,
        if is_dev_network {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(15)
        },
    );

    let peer_cache_connector = PeerCacheConnector::new(
        ledger.clone(),
        peer_connector.clone(),
        stats.clone(),
        config.network.cached_peer_reachout,
    );
    if !config.network.peer_reachout.is_zero() {
        ticker_pool.insert(peer_cache_connector, config.network.cached_peer_reachout);
    }

    let monitor = NodeMonitor::new(
        ledger.clone(),
        network.clone(),
        online_reps.clone(),
        active_elections.clone(),
        block_rates.clone(),
    );
    if config.enable_monitor {
        ticker_pool.insert(monitor, config.monitor.interval)
    }

    let wallets_ticker = WalletsTicker(wallets.clone());
    ticker_pool.insert(wallets_ticker, Duration::from_millis(500));

    let mut wallet_reps_checker = WalletRepsChecker::new(wallet_reps.clone());
    wallet_reps_checker.add_consumer(vote_rebroadcast_queue.clone());
    ticker_pool.insert(
        wallet_reps_checker,
        if is_dev_network {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(60)
        },
    );

    let rep_tiers = Arc::new(CurrentRepTiers::new());
    let mut rep_tiers_calculator =
        RepTiersCalculator::new(rep_weights.clone(), online_reps.clone(), stats.clone());
    rep_tiers_calculator.add_tiers_consumer(vote_processor_queue.clone());
    rep_tiers_calculator.add_tiers_consumer(vote_rebroadcast_queue.clone());
    rep_tiers_calculator.add_tiers_consumer(rep_tiers.clone());
    ticker_pool.insert(
        rep_tiers_calculator,
        if is_dev_network {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(10)
        },
    );

    let wallet_backup = WalletBackup {
        data_path: application_path.clone(),
        wallets: wallets.clone(),
    };
    if !flags.disable_backup {
        ticker_pool.insert(wallet_backup, Duration::from_secs(60 * 5));
    }

    let receivable_search = ReceivableSearch::new(wallets.clone());
    if !flags.disable_search_pending {
        ticker_pool.insert(
            receivable_search,
            if is_dev_network {
                Duration::from_secs(1)
            } else {
                Duration::from_secs(5)
            },
        );
    }

    let local_reps_computation = LocalRepsComputation::new(wallet_reps.clone());
    ticker_pool.insert(
        local_reps_computation,
        if is_dev_network {
            Duration::from_millis(10)
        } else {
            Duration::from_secs(10)
        },
    );

    let ticker_services = TickerServices::new(ticker_pool);

    let message_flooder = Arc::new(Mutex::new(message_flooder.clone()));

    let recently_cemented_inserter = RecentlyCementedInserter {
        recently_cemented: recently_cemented.clone(),
    };

    let bootstrap_election_activator = BootstrapElectionActivator {
        active_elections: active_elections.clone(),
        vote_cache: vote_cache.clone(),
        stats: stats.clone(),
    };

    let local_votes_remover = LocalVotesRemover {
        active_elections: active_elections.clone(),
        vote_history: vote_history.clone(),
    };

    let aec_fork_inserter = Arc::new(AecForkInserter {
        rep_weights: rep_weights.clone(),
        fork_cache: fork_cache.clone(),
        active_elections: active_elections.clone(),
        vote_cache: vote_cache.clone(),
    });

    let aec_voter = AecVoter::new(
        active_elections.clone(),
        vote_generators.clone(),
        steady_clock.clone(),
        current_network,
        cps_limiter,
    );

    // With ledger_snapshots we never vote for forked blocks!
    #[cfg(not(feature = "ledger_snapshots"))]
    {
        use crate::consensus::ForkInserterPlugin;

        ledger_event_processor_plugins
            .push(Box::new(ForkInserterPlugin::new(aec_fork_inserter.clone())));
    }

    #[cfg(feature = "ledger_snapshots")]
    {
        ledger_event_processor_plugins.push(Box::new(ForkDetector::new(
            ledger.clone(),
            ledger_snapshots.clone(),
            active_elections.clone(),
        )));
    }

    let aec_event_processor = AecEventProcessor {
        node_observer: node_observer.clone(),
        election_schedulers: election_schedulers.clone(),
        network_filter: network_filter.clone(),
        bootstrap_election_activator,
        recently_cemented_inserter,
        vote_cache_processor: vote_cache_processor.clone(),
        vote_cache: vote_cache.clone(),
        vote_rebroadcast_queue: vote_rebroadcast_queue.clone(),
        vote_processor: vote_processor.clone(),
        block_processor_queue: block_processor_queue.clone(),
        confirming_set: confirming_set.clone(),
        online_reps: online_reps.clone(),
        active_elections: active_elections.clone(),
        rep_crawler: rep_crawler.clone(),
        clock: steady_clock.clone(),
        local_votes_remover,
        aec_fork_inserter,
        stats: stats.clone(),
        winner_block_broadcaster: winner_block_broadcaster.clone(),
        plugins: Vec::new(),
    };

    spawn_backpressure_processor("AEC ev proc", aec_receiver, aec_event_processor);

    let dependent_elections_confirmer = DependentElectionsConfirmer {
        confirming_set: confirming_set.clone(),
        active_elections: active_elections.clone(),
        clock: steady_clock.clone(),
    };

    let fork_cache_updater = ForkCacheUpdater::new(fork_cache.clone());

    let ledger_event_processor = LedgerEventProcessor {
        node_event_sender: node_observer.clone(),
        dependent_elections_confirmer,
        confirming_set: confirming_set.clone(),
        stats: stats.clone(),
        bootstrapper: bootstrapper.clone(),
        vote_history: vote_history.clone(),
        active_elections: active_elections.clone(),
        block_processor_queue: block_processor_queue.clone(),
        bounded_backlog: bounded_backlog.clone(),
        fork_cache_updater,
        plugins: ledger_event_processor_plugins,
    };

    spawn_backpressure_processor("Ledger ev proc", ledger_rx, ledger_event_processor);

    vote_processor.add_observer(aec_sender);

    let mut stats_collector = StatsCollector::new();
    stats_collector.add_source(stats.clone());
    stats_collector.add_source(ledger.clone());
    stats_collector.add_source(online_reps.clone());
    stats_collector.add_source(fork_cache.clone());
    stats_collector.add_source(active_elections.clone());
    let vote_rebroadcaster_stats = vote_rebroadcaster.lock().unwrap().stats.clone();
    stats_collector.add_source(vote_rebroadcaster_stats);
    stats_collector.add_source(election_schedulers.clone());
    stats_collector.add_source(network.clone());
    stats_collector.add_source(backlog_scan.stats());
    stats_collector.add_source(handshake_stats);
    stats_collector.add_source(inbound_message_queue.clone());
    stats_collector.add_source(bootstrap_stale_stats);
    stats_collector.add_source(block_processor.clone());
    stats_collector.add_source(block_processor_queue.clone());
    stats_collector.add_source(backlog_waiter.clone());
    stats_collector.add_source(conf_time_stats);
    stats_collector.add_source(winner_block_broadcaster.clone());
    stats_collector.add_source(bootstrapper.clone());
    stats_collector.add_source(unchecked.clone());
    stats_collector.add_source(unchecked_reenqueuer.stats().clone());

    let backlog_scan = BacklogServices::new(backlog_scan);

    let mut container_info = ContainerInfoFactory::new();
    container_info.add("work", work_factory.clone());
    container_info.add("ledger", ledger.clone());
    container_info.add("active", active_elections.clone());
    container_info.add("network", network.clone());
    container_info.add("syn_cookies", syn_cookies);
    container_info.add("telemetry", telemetry.clone());
    container_info.add("wallets", wallets.clone());
    container_info.add("vote_processor", vote_processor_queue.clone());
    container_info.add("vote_cache_processor", vote_cache_processor.clone());
    container_info.add("rep_crawler", rep_crawler.clone());
    container_info.add("block_processor", block_processor_queue.clone());
    container_info.add("online_reps", online_reps.clone());
    container_info.add("history", vote_history.clone());
    container_info.add("confirming_set", confirming_set.clone());
    container_info.add("request_aggregator", request_aggregator.clone());
    container_info.add("election_scheduler", election_schedulers.clone());
    container_info.add("vote_cache", vote_cache.clone());
    container_info.add("vote_generators", vote_generators.clone());
    container_info.add("bootstrapper", bootstrapper.clone());
    container_info.add("unchecked", unchecked.clone());
    container_info.add("local_block_broadcaster", local_block_broadcaster.clone());
    container_info.add("rep_tiers", rep_tiers.clone());
    container_info.add("inbound_msg_queue", inbound_message_queue.clone());
    container_info.add("bounded_backlog", bounded_backlog.clone());
    container_info.add("vote_rebroadcaster", vote_rebroadcast_queue.clone());
    container_info.add("fork_cache", fork_cache.clone());
    container_info.add("event_queues", event_queues_info);

    let wallet_services =
        WalletServices::new(wallets.clone(), work_factory.clone(), wallet_reps.clone());
    let ledger_query_services = LedgerQueryServices::new(
        ledger.clone(),
        block_rates.clone(),
        confirming_set.clone(),
        recently_cemented.clone(),
        stats.clone(),
    );
    let bootstrap_work_services =
        BootstrapWorkServices::new(bootstrapper.clone(), bootstrap_server.clone(), work_factory.clone());

    Ok(ComposedNode {
        is_nulled,
        runtime,
        data_path: application_path,
        node_id: node_id_key,
        config,
        network_params,
        workers,
        flags,
        network,
        tcp_listener,
        peer_connector,
        network_threads,
        message_processor,
        message_sender: message_publisher_l,
        message_flooder,
        keepalive_publisher,
        inbound_message_queue,
        network_filter,
        steady_clock,
        active: active_elections,
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
        telemetry,
        bootstrap_server,
        work_factory,
        stats,
        ledger,
        wallet_services,
        ledger_query_services,
        bootstrap_work_services,
        unchecked,
        backlog_scan,
        tokio_runner,
        aec_ticker: Arc::new(TimerThread::new("AEC ticker", aec_ticker)),
        stats_collector,
        container_info_factory: container_info,
        aec_voter: Arc::new(TimerThread::new("AEC voter", aec_voter)),
        ticker_services,
        #[cfg(feature = "ledger_snapshots")]
        ledger_snapshots,
    })
}
