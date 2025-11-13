//! Composition helpers for building the node graph in distinct phases.

use std::{
    fs::Permissions,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

use anyhow::Context;
use num_format::{Locale, ToFormattedString};
use rsnano_ledger::{Ledger, LedgerBuilder};
use rsnano_messages::{Message, NetworkFilter};
use rsnano_network::{Network, PeerConnector, TcpListener, TcpNetworkAdapter};
use rsnano_network_protocol::{
    HandshakeStats, InboundMessageQueue, LatestKeepalives, NanoDataReceiverFactory, SynCookies,
};
use rsnano_nullable_clock::SteadyClock;
use rsnano_nullable_fs::NullableFilesystem;
use rsnano_store_lmdb::{LmdbLedgerStoreFactory, LmdbWalletEnvironmentFactory};
use rsnano_types::{Networks, NodeId, Peer, PrivateKey};
use rsnano_utils::{
    container_info::ContainerInfoFactory,
    stats::Stats,
    sync::backpressure_channel::{self, Receiver, Sender},
    thread_pool::ThreadPool,
    ticker::TickerPool,
};
use tracing::info;

use crate::{
    block_processing::{LedgerEvent, UncheckedMap},
    bootstrap::BootstrapServer,
    config::{GlobalConfig, NetworkParams, NodeConfig, NodeFlags},
    consensus::{ActiveElectionsContainer, get_bootstrap_weights, log_bootstrap_weights},
    node_id_key_file::NodeIdKeyFile,
    representatives::{OnlineReps, RepCrawler},
    telemetry::{
        TelementryConfig, Telemetry, TelemetryFactory, rsnano_build_info, rsnano_version_string,
    },
    tokio_runner::TokioRunner,
    transport::{
        MessageSender,
        keepalive::{KeepaliveMessageFactory, KeepalivePublisher},
    },
};

pub(crate) struct FoundationBits {
    pub(crate) tokio_runner: TokioRunner,
    pub(crate) runtime: tokio::runtime::Handle,
    pub(crate) config: NodeConfig,
    pub(crate) network_params: NetworkParams,
    pub(crate) flags: NodeFlags,
    pub(crate) application_path: PathBuf,
    pub(crate) steady_clock: Arc<SteadyClock>,
    pub(crate) stats: Arc<Stats>,
    pub(crate) node_id_key: PrivateKey,
    pub(crate) node_id: NodeId,
    pub(crate) global_config: GlobalConfig,
    pub(crate) ledger: Arc<Ledger>,
    pub(crate) ledger_tx: Sender<LedgerEvent>,
    pub(crate) ledger_rx: Receiver<LedgerEvent>,
    pub(crate) event_queues_info: ContainerInfoFactory,
    pub(crate) workers: Arc<ThreadPool>,
    pub(crate) ticker_pool: TickerPool,
    pub(crate) current_network: Networks,
    pub(crate) wallet_env_factory: LmdbWalletEnvironmentFactory,
    pub(crate) syn_cookies: Arc<SynCookies>,
}

pub(crate) struct NetworkIoBits {
    pub(crate) network_adapter: Arc<TcpNetworkAdapter>,
    pub(crate) peer_connector: Arc<PeerConnector>,
    pub(crate) keepalive_factory: Arc<KeepaliveMessageFactory>,
    pub(crate) keepalive_publisher: Arc<KeepalivePublisher>,
    pub(crate) rep_crawler: Arc<RepCrawler>,
    pub(crate) tcp_listener: Arc<TcpListener>,
}

pub(crate) struct TelemetryBits {
    pub(crate) telemetry: Arc<Telemetry>,
    pub(crate) bootstrap_server: Arc<BootstrapServer>,
    pub(crate) data_receiver_factory: Box<NanoDataReceiverFactory>,
    pub(crate) latest_keepalives: Arc<Mutex<LatestKeepalives>>,
    pub(crate) handshake_stats: Arc<HandshakeStats>,
}

pub(crate) fn build_foundation(
    mut config: NodeConfig,
    network_params: NetworkParams,
    flags: NodeFlags,
    application_path: PathBuf,
    is_nulled: bool,
    node_id_key_file: &mut NodeIdKeyFile,
) -> anyhow::Result<FoundationBits> {
    let mut tokio_runner = TokioRunner::new(config.io_threads);
    tokio_runner.start();
    let runtime = tokio_runner.handle().clone();

    let current_network = network_params.network.current_network;
    let network_label = network_params.network.get_current_network_as_string();

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

    if flags.enable_voting {
        config.enable_voting = true;
    }

    let steady_clock = if is_nulled {
        Arc::new(SteadyClock::new_null())
    } else {
        Arc::new(SteadyClock::default())
    };

    let stats = Arc::new(Stats::new(Default::default()));

    let global_config = GlobalConfig {
        node_config: config.clone(),
        flags: flags.clone(),
        network_params: network_params.clone(),
    };

    let node_id_key = node_id_key_file
        .initialize(&application_path)
        .context("Failed to initialize node ID key file")?;
    let node_id = NodeId::from(&node_id_key);
    info!("Node ID: {}", node_id);

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
            .with_context(|| format!("Could not create data dir {:?}", application_path))?;
        fs.set_permissions(&application_path, Permissions::from_mode(0o700))
            .with_context(|| {
                format!(
                    "Could not set data dir permissions for {:?}",
                    application_path
                )
            })?;
    }

    let mut ledger_path = application_path.clone();
    ledger_path.push("data.ldb");

    let lmdb_store_factory = if is_nulled {
        LmdbLedgerStoreFactory::new_null()
    } else {
        LmdbLedgerStoreFactory::default()
    };

    let wallet_env_factory = if is_nulled {
        LmdbWalletEnvironmentFactory::new_null()
    } else {
        LmdbWalletEnvironmentFactory::default()
    };

    info!("LMDB sync strategy: {:?}", config.lmdb_config.sync);
    info!("Loading ledger, this may take a while...");
    let ledger = LedgerBuilder::new(&ledger_path)
        .store_factory(&lmdb_store_factory)
        .config(config.lmdb_config.clone())
        .constants(network_params.ledger.clone())
        .min_rep_weight(config.representative_vote_weight_minimum)
        .bootstrap_weights(bootstrap_weights)
        .stats(stats.clone())
        .finish()
        .with_context(|| format!("Could not open ledger at {:?}", ledger_path))?;

    info!("Database backend: {}", ledger.store_vendor());

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
        ledger.rep_weights.len().to_formatted_string(&Locale::en)
    );

    log_bootstrap_weights(&ledger.rep_weights);

    let workers = Arc::new(ThreadPool::new(
        config.background_threads as usize,
        "Worker".to_string(),
    ));
    let ticker_pool = TickerPool::with_thread_pool(workers.clone());

    let syn_cookies = Arc::new(SynCookies::new(network_params.network.max_peers_per_ip));

    Ok(FoundationBits {
        tokio_runner,
        runtime,
        config,
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
        event_queues_info,
        workers,
        ticker_pool,
        current_network,
        wallet_env_factory,
        syn_cookies,
    })
}

pub(crate) fn build_network_io(
    config: &NodeConfig,
    network_params: &NetworkParams,
    runtime: &tokio::runtime::Handle,
    steady_clock: &Arc<SteadyClock>,
    stats: &Arc<Stats>,
    network: &Arc<RwLock<Network>>,
    ledger: &Arc<Ledger>,
    message_sender: &MessageSender,
    online_reps: &Arc<Mutex<OnlineReps>>,
    active_elections: &Arc<RwLock<ActiveElectionsContainer>>,
) -> NetworkIoBits {
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

    let tcp_listener = Arc::new(TcpListener::new(
        network.read().unwrap().listening_port(),
        network_adapter.clone(),
        runtime.clone(),
    ));

    NetworkIoBits {
        network_adapter,
        peer_connector,
        keepalive_factory,
        keepalive_publisher,
        rep_crawler,
        tcp_listener,
    }
}

pub(crate) fn build_telemetry_bits(
    config: &NodeConfig,
    flags: &NodeFlags,
    network_params: &NetworkParams,
    network: &Arc<RwLock<Network>>,
    inbound_queue: &Arc<InboundMessageQueue>,
    network_filter: &Arc<NetworkFilter>,
    stats: &Arc<Stats>,
    syn_cookies: &Arc<SynCookies>,
    node_id_key: &PrivateKey,
    message_sender: &MessageSender,
    steady_clock: &Arc<SteadyClock>,
    ledger: &Arc<Ledger>,
    unchecked: &Arc<Mutex<UncheckedMap>>,
) -> TelemetryBits {
    let latest_keepalives = Arc::new(Mutex::new(LatestKeepalives::default()));
    let handshake_stats = Arc::new(HandshakeStats::default());

    let inbound_queue_clone = inbound_queue.clone();
    let try_enqueue = Arc::new(move |msg: Message, channel| inbound_queue_clone.put(msg, channel));

    let data_receiver_factory = Box::new(NanoDataReceiverFactory::new(
        network,
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

    TelemetryBits {
        telemetry,
        bootstrap_server,
        data_receiver_factory,
        latest_keepalives,
        handshake_stats,
    }
}
