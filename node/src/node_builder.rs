use std::{
    fs::Permissions,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock,
        mpsc::{self, SyncSender},
    },
    time::Duration,
};

use bounded_vec_deque::BoundedVecDeque;
use num_format::{Locale, ToFormattedString};
use tracing::{info, warn};

use rsnano_ledger::LedgerBuilder;
use rsnano_messages::{Message, NetworkFilter};
use rsnano_network::{
    ChannelId, DeadChannelCleanup, Network, NetworkCleanup, PeerConnector, TcpListener,
    TcpNetworkAdapter, TrafficType,
};
use rsnano_network_protocol::{
    HandshakeStats, InboundMessageQueue, InboundMessageQueueCleanup, LatestKeepalives,
    LatestKeepalivesCleanup, MessageCallback, NanoDataReceiverFactory, SynCookies,
};
use rsnano_nullable_clock::{SteadyClock, SystemTimeFactory};
use rsnano_nullable_fs::NullableFilesystem;
use rsnano_nullable_lmdb::{
    EnvironmentFlags, EnvironmentOptions, LmdbEnvironment, LmdbEnvironmentFactory,
};
use rsnano_types::{Networks, NodeId, Peer, PrivateKey};
use rsnano_utils::{
    CancellationToken,
    container_info::ContainerInfoFactory,
    get_cpu_count,
    stats::{Stats, StatsCollector},
    sync::backpressure_channel,
    thread_pool::ThreadPool,
    ticker::{Tickable, TickerPool, TimerThread},
};
use rsnano_wallet::{ReceivableSearch, WalletBackup, Wallets, WalletsTicker};

#[cfg(feature = "ledger_snapshots")]
use crate::ledger_snapshots::{LedgerSnapshots, fork_detector::ForkDetector};
use crate::{
    BacklogServices, Node, NodeArgs, NodeEvent, NodeServices, OnlineWeightSampler, TickerServices,
    aec_event_processor::AecEventProcessor,
    block_processing::{
        BacklogScan, BacklogWaiter, BlockProcessor, BlockProcessorQueue, BoundedBacklog,
        BoundedBacklogPlugin, LocalBlockBroadcaster, LocalBlockBroadcasterPlugin,
        ProcessQueueConfig, UncheckedBlockReenqueuer, UncheckedMap,
    },
    block_rate_calculator::BlockRateCalculator,
    bootstrap::{BootstrapResponderCleanup, BootstrapServer, Bootstrapper, BootstrapperCleanup},
    cementation::{ConfirmingSet, TrackConfirmationTimes},
    config::{
        DaemonConfig, DaemonToml, GlobalConfig, NetworkParams, NodeConfig, NodeFlags,
        get_node_toml_config_path,
    },
    consensus::{
        ActiveElectionsContainer, AecForkInserter, AecTicker, AecVoter, BootstrapElectionActivator,
        BootstrapStaleElections, ConfirmReqSender, ConfirmationSolicitorPlugin, CpsLimiter,
        CurrentRepTiers, DependentElectionsConfirmer, ForkCache, ForkCacheUpdater,
        LocalVoteHistory, LocalVotesRemover, RepTiersCalculator, RequestAggregator,
        RequestAggregatorCleanup, VoteApplier, VoteBroadcaster, VoteCache, VoteCacheProcessor,
        VoteGenerators, VoteProcessor, VoteProcessorQueue, VoteProcessorQueueCleanup,
        VoteRebroadcastQueue, VoteRebroadcaster, WalletRepsChecker, WinnerBlockBroadcaster,
        election_schedulers::{ElectionSchedulers, ElectionSchedulersPlugin},
        get_bootstrap_weights, log_bootstrap_weights,
    },
    ledger_event_processor::{LedgerEventProcessor, LedgerEventProcessorPlugin},
    node_id_key_file::NodeIdKeyFile,
    node_monitor::NodeMonitor,
    recently_cemented_inserter::RecentlyCementedInserter,
    representatives::{OnlineReps, OnlineRepsCleanup, OnlineWeightCalculation, RepCrawler},
    telemetry::{
        TelementryConfig, Telemetry, TelemetryFactory, rsnano_build_info, rsnano_version_string,
    },
    tokio_runner::TokioRunner,
    transport::{
        MessageFlooder, MessageProcessor, MessageSender, NetworkMessageProcessor, NetworkThreads,
        PeerCacheConnector, PeerCacheUpdater,
        keepalive::{KeepaliveMessageFactory, KeepalivePublisher},
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
}

pub(crate) struct NodeParts {
    pub(crate) is_nulled: bool,
    pub(crate) runtime: tokio::runtime::Handle,
    pub(crate) data_path: PathBuf,
    pub(crate) node_id: PrivateKey,
    pub(crate) config: NodeConfig,
    pub(crate) network_params: NetworkParams,
    pub(crate) workers: Arc<ThreadPool>,
    pub(crate) flags: NodeFlags,
    pub(crate) services: NodeServices,
    pub(crate) unchecked: Arc<Mutex<UncheckedMap>>,
    pub(crate) backlog_scan: BacklogServices,
    pub(crate) tokio_runner: TokioRunner,
    pub(crate) aec_ticker: TimerThread<AecTicker>,
    pub(crate) stats_collector: StatsCollector,
    pub(crate) container_info_factory: ContainerInfoFactory,
    pub(crate) aec_voter: TimerThread<AecVoter>,
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

        let config = match self.config {
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

        Ok(Node::new_with_args(args))
    }
}
pub(crate) fn build_node_parts(
    args: NodeArgs,
    is_nulled: bool,
    mut node_id_key_file: NodeIdKeyFile,
) -> NodeParts {
    let mut tokio_runner = TokioRunner::new(args.config.io_threads);
    tokio_runner.start();
    let runtime = tokio_runner.handle().clone();

    let network_params = args.network_params;
    let current_network = network_params.network.current_network;
    let network_label = network_params.network.get_current_network_as_string();
    let application_path = args.data_path;

    info!("Node started");
    info!("Version: {}", rsnano_version_string());
    info!("{}", rsnano_build_info());
    info!("Network: {}", network_label);
    info!("Data path: {:?}", application_path);
    info!(
        "Genesis block: {}",
        network_params.ledger.genesis_block.hash()
    );
    info!(
        "Genesis account: {}",
        network_params.ledger.genesis_account.encode_account()
    );

    let mut config = args.config;
    let flags = args.flags;
    if flags.enable_voting {
        config.enable_voting = true;
    }

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

    let node_observer = args.event_sender;
    // Time relative to the start of the node. This makes time exlpicit and enables us to
    // write time relevant unit tests with ease.
    let steady_clock = if is_nulled {
        Arc::new(SteadyClock::new_null())
    } else {
        Arc::new(SteadyClock::default())
    };

    let global_config = &GlobalConfig {
        node_config: config.clone(),
        flags: flags.clone(),
        network_params: network_params.clone(),
    };
    let node_id_key = node_id_key_file.initialize(&application_path).unwrap();
    let node_id = NodeId::from(&node_id_key);
    info!("Node ID: {}", node_id);

    let stats = Arc::new(Stats::new(Default::default()));

    let bootstrap_weights = if (network_params.network.is_live_network()
        || network_params.network.is_beta_network())
        && !flags.inactive_node
    {
        get_bootstrap_weights(current_network)
    } else {
        Default::default()
    };

    let fs = if is_nulled {
        NullableFilesystem::new_null()
    } else {
        NullableFilesystem::default()
    };

    if !fs.exists(&application_path) {
        fs.create_dir_all(&application_path)
            .expect("Could not create data dir");
        fs.set_permissions(&application_path, Permissions::from_mode(0o700))
            .expect("Could not set data dir permissions");
    }

    let mut ledger_path = application_path.clone();
    ledger_path.push("data.ldb");

    let lmdb_env_factory = if is_nulled {
        LmdbEnvironmentFactory::new_null()
    } else {
        LmdbEnvironmentFactory::default()
    };

    info!("LMDB sync strategy: {:?}", config.lmdb_config.sync);
    info!("Loading ledger, this may take a while...");
    let ledger = LedgerBuilder::new(&ledger_path)
        .env_factory(&lmdb_env_factory)
        .config(config.lmdb_config.clone())
        .constants(network_params.ledger.clone())
        .min_rep_weight(config.representative_vote_weight_minimum)
        .bootstrap_weights(bootstrap_weights)
        .stats(stats.clone())
        .finish();

    let ledger = match ledger {
        Ok(i) => i,
        Err(e) => {
            panic!("Could not open ledger: {:?}. Details: {:?}", ledger_path, e)
        }
    };

    // hard coded version! TODO: read version from Cargo
    info!("Database backend: {}", ledger.store_vendor());

    let rep_weights = ledger.rep_weights.clone();

    let mut event_queues_info = ContainerInfoFactory::new();
    let (ledger_tx, ledger_rx) = backpressure_channel::channel(1024);
    let ledger_tx_clone = ledger_tx.clone();
    event_queues_info.add_leaf("ledger", move || ledger_tx_clone.len());

    let ledger = Arc::new(ledger);
    info!(
        "Block count:     {}",
        ledger.block_count().to_formatted_string(&Locale::en)
    );
    info!(
        "Confirmed count: {}",
        ledger.confirmed_count().to_formatted_string(&Locale::en)
    );
    info!(
        "Account count:   {}",
        ledger.account_count().to_formatted_string(&Locale::en)
    );
    info!(
        "Representative count: {}",
        rep_weights.len().to_formatted_string(&Locale::en)
    );

    log_bootstrap_weights(&rep_weights);

    let mut ledger_event_processor_plugins: Vec<Box<dyn LedgerEventProcessorPlugin>> = Vec::new();

    let syn_cookies = Arc::new(SynCookies::new(network_params.network.max_peers_per_ip));

    let workers = Arc::new(ThreadPool::new(
        config.background_threads as usize,
        "Worker".to_string(),
    ));
    let mut ticker_pool = TickerPool::with_thread_pool(workers.clone());

    let mut inbound_message_queue = InboundMessageQueue::new(config.message_processor.max_queue);
    if let Some(cb) = args.callbacks.on_inbound {
        inbound_message_queue.set_inbound_callback(cb);
    }
    if let Some(cb) = args.callbacks.on_inbound_dropped {
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
    // Make sure that online weight is properly calculated from the beginning;
    online_weight_calculation.tick(&CancellationToken::new());
    ticker_pool.insert(
        online_weight_calculation,
        OnlineReps::default_interval_for(current_network),
    );

    let mut message_sender =
        MessageSender::new(stats.clone(), network_params.network.protocol_info());

    if let Some(callback) = &args.callbacks.on_publish {
        message_sender.set_published_callback(callback.clone());
    }

    let message_flooder = MessageFlooder::new(
        online_reps.clone(),
        network.clone(),
        stats.clone(),
        message_sender.clone(),
    );

    let telemetry_config = TelementryConfig {
        enable_ongoing_broadcasts: !flags.disable_providing_telemetry_metrics,
    };
    let telemetry_factory = TelemetryFactory {
        ledger: ledger.clone(),
        network: network.clone(),
        node_id_key: node_id_key.clone(),
        unchecked: unchecked.clone(),
        startup_time: steady_clock.now(),
        clock: steady_clock.clone(),
    };
    let telemetry = Arc::new(Telemetry::new(
        telemetry_factory,
        telemetry_config,
        stats.clone(),
        ledger.genesis().hash(),
        network_params.clone(),
        network.clone(),
        message_sender.clone(),
        steady_clock.clone(),
    ));

    let bootstrap_server = Arc::new(BootstrapServer::new(
        config.bootstrap_server.clone(),
        stats.clone(),
        ledger.clone(),
        steady_clock.clone(),
        message_sender.clone(),
    ));

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
    ticker_pool.insert(unchecked_reenqueuer.clone(), Duration::from_secs(1));

    let mut wallets_path = application_path.clone();
    wallets_path.push("wallets.ldb");

    let wallets_env = if is_nulled {
        Arc::new(LmdbEnvironment::new_null())
    } else {
        let options = EnvironmentOptions {
            path: wallets_path,
            max_dbs: 128,
            map_size: 1024 * 1024 * 1024,
            flags: EnvironmentFlags::NO_SUB_DIR
                | EnvironmentFlags::NO_TLS
                | EnvironmentFlags::NO_READAHEAD,
        };
        Arc::new(
            lmdb_env_factory
                .create(options)
                .expect("Could not create LMDB env for wallets"),
        )
    };

    let wallets_config = global_config.wallets_config();

    let mut wallets = Wallets::new(
        wallets_config.clone(),
        wallets_env,
        ledger.clone(),
        network_params.work.clone(),
        steady_clock.clone(),
    );
    if !is_nulled {
        wallets.initialize().expect("Could not create wallet");
    }

    let wallets = Arc::new(wallets);

    let (tx_work, rx_work) = mpsc::channel();
    wallets.set_work_queue(tx_work);

    let (tx_block, rx_block) = mpsc::channel();
    wallets.set_block_queue(tx_block);

    let wallet_work = WalletWorkProvider::new(wallets.clone(), rx_work, work_factory.clone());

    std::thread::Builder::new()
        .name("Wallet work".to_owned())
        .spawn(move || {
            wallet_work.run();
        })
        .unwrap();

    let wallet_blocks =
        WalletBlockProcessor::new(rx_block, wallets.clone(), block_processor_queue.clone());

    std::thread::Builder::new()
        .name("Wallet blocks".to_owned())
        .spawn(move || wallet_blocks.run())
        .unwrap();

    let wallet_reps = Arc::new(Mutex::new(WalletRepresentatives::new(
        wallets_config.voting_enabled,
        wallets_config.vote_minimum,
        ledger.rep_weights.clone(),
        wallets.clone(),
        online_reps.clone(),
    )));
    wallet_reps.lock().unwrap().compute_reps();

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
        &config,
        &network_params,
        vote_broadcaster,
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
        network.clone(),
    )));

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

    let mut bootstrap_sender = MessageSender::new_with_buffer_size(
        stats.clone(),
        network_params.network.protocol_info(),
        512,
    );

    if let Some(callback) = &args.callbacks.on_publish {
        bootstrap_sender.set_published_callback(callback.clone());
    }

    let latest_keepalives = Arc::new(Mutex::new(LatestKeepalives::default()));
    let handshake_stats = Arc::new(HandshakeStats::default());

    let inbound_queue_clone = inbound_message_queue.clone();
    let try_enqueue = Arc::new(move |msg, channel| inbound_queue_clone.put(msg, channel));

    let data_receiver_factory = Box::new(NanoDataReceiverFactory::new(
        &network,
        try_enqueue,
        network_filter.clone(),
        stats.clone(),
        handshake_stats.clone(),
        syn_cookies.clone(),
        node_id_key.clone(),
        latest_keepalives.clone(),
        network_params.ledger.genesis_block.hash(),
        network_params.network.protocol_info(),
    ));

    network
        .write()
        .unwrap()
        .set_data_receiver_factory(data_receiver_factory);

    let network_adapter = Arc::new(TcpNetworkAdapter::new(
        network.clone(),
        steady_clock.clone(),
        runtime.clone(),
    ));

    let peer_connector = Arc::new(PeerConnector::new(
        config.tcp.connect_timeout,
        network_adapter.clone(),
        runtime.clone(),
    ));

    let keepalive_factory = Arc::new(KeepaliveMessageFactory::new(
        network.clone(),
        Peer::new(config.external_address.clone(), config.external_port),
    ));

    let keepalive_publisher = Arc::new(KeepalivePublisher::new(
        network.clone(),
        peer_connector.clone(),
        message_sender.clone(),
        keepalive_factory.clone(),
    ));

    let rep_crawler = Arc::new(RepCrawler::new(
        online_reps.clone(),
        stats.clone(),
        config.rep_crawler_query_timeout,
        config.clone(),
        network_params.clone(),
        network.clone(),
        ledger.clone(),
        steady_clock.clone(),
        message_sender.clone(),
        keepalive_publisher.clone(),
        active_elections.clone(),
        runtime.clone(),
    ));

    // BEWARE: `bootstrap` takes `network.port` instead of `config.peering_port` because when the user doesn't specify
    //         a peering port and wants the OS to pick one, the picking happens when `network` gets initialized
    //         (if UDP is active, otherwise it happens when `bootstrap` gets initialized), so then for TCP traffic
    //         we want to tell `bootstrap` to use the already picked port instead of itself picking a different one.
    //         Thus, be very careful if you change the order: if `bootstrap` gets constructed before `network`,
    //         the latter would inherit the port from the former (if TCP is active, otherwise `network` picks first)
    //
    let tcp_listener = Arc::new(TcpListener::new(
        network.read().unwrap().listening_port(),
        network_adapter.clone(),
        runtime.clone(),
    ));

    let request_aggregator = Arc::new(RequestAggregator::new(
        config.request_aggregator.clone(),
        stats.clone(),
        vote_generators.clone(),
        ledger.clone(),
    ));

    let mut backlog_scan =
        BacklogScan::new(global_config.into(), ledger.clone(), steady_clock.clone());

    //  TODO: Hook this direclty in the schedulers
    let schedulers_w = Arc::downgrade(&election_schedulers);
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

    if config.enable_bounded_backlog {
        info!(
            "Bounded backlog enabled: max backlog={}, batch_size={}, scan_rate={}",
            config.bounded_backlog.max_backlog,
            config.bounded_backlog.batch_size,
            config.bounded_backlog.scan_rate
        );

        ledger_event_processor_plugins
            .push(Box::new(BoundedBacklogPlugin::new(bounded_backlog.clone())));

        // Activate accounts with unconfirmed blocks
        let backlog_w = Arc::downgrade(&bounded_backlog);
        backlog_scan.on_unconfirmed_found(move |batch| {
            if let Some(backlog) = backlog_w.upgrade() {
                backlog.activate_batch(batch);
            }
        });

        // Erase accounts with all confirmed blocks
        let backlog_w = Arc::downgrade(&bounded_backlog);
        backlog_scan.on_up_to_date(move |batch| {
            if let Some(backlog) = backlog_w.upgrade() {
                backlog.erase_accounts(batch);
            }
        });
    }

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

    let rep_crawler_w = Arc::downgrade(&rep_crawler);
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

    let keepalive_factory_w = Arc::downgrade(&keepalive_factory);
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

    let services = NodeServices {
        steady_clock: steady_clock.clone(),
        stats: stats.clone(),
        work_factory: work_factory.clone(),
        ledger: ledger.clone(),
        network: network.clone(),
        telemetry: telemetry.clone(),
        bootstrap_server: bootstrap_server.clone(),
        online_reps: online_reps.clone(),
        rep_tiers: rep_tiers.clone(),
        vote_processor_queue: vote_processor_queue.clone(),
        vote_history: vote_history.clone(),
        confirming_set: confirming_set.clone(),
        vote_cache: vote_cache.clone(),
        vote_cache_processor: vote_cache_processor.clone(),
        block_processor: block_processor.clone(),
        block_processor_queue: block_processor_queue.clone(),
        wallets: wallets.clone(),
        vote_generators: vote_generators.clone(),
        active: active_elections.clone(),
        vote_processor: vote_processor.clone(),
        rep_crawler: rep_crawler.clone(),
        tcp_listener: tcp_listener.clone(),
        election_schedulers: election_schedulers.clone(),
        request_aggregator: request_aggregator.clone(),
        bounded_backlog: bounded_backlog.clone(),
        bootstrapper: bootstrapper.clone(),
        local_block_broadcaster: local_block_broadcaster.clone(),
        network_threads: network_threads.clone(),
        peer_connector: peer_connector.clone(),
        inbound_message_queue: inbound_message_queue.clone(),
        network_filter: network_filter.clone(),
        message_processor: message_processor.clone(),
        message_sender: message_publisher_l.clone(),
        message_flooder: message_flooder.clone(),
        keepalive_publisher: keepalive_publisher.clone(),
        recently_cemented: recently_cemented.clone(),
        block_rates: block_rates.clone(),
        wallet_reps: wallet_reps.clone(),
        vote_rebroadcaster: vote_rebroadcaster.clone(),
        winner_block_broadcaster: winner_block_broadcaster.clone(),
        #[cfg(feature = "ledger_snapshots")]
        ledger_snapshots: ledger_snapshots.clone(),
    };

    NodeParts {
        is_nulled,
        runtime,
        data_path: application_path,
        node_id: node_id_key,
        config,
        network_params,
        workers,
        flags,
        services,
        unchecked,
        backlog_scan,
        tokio_runner,
        aec_ticker: TimerThread::new("AEC ticker", aec_ticker),
        stats_collector,
        container_info_factory: container_info,
        aec_voter: TimerThread::new("AEC voter", aec_voter),
        ticker_services,
        #[cfg(feature = "ledger_snapshots")]
        ledger_snapshots,
    }
}
