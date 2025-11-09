//! Composition helpers for building the node graph in distinct phases.

use std::{fs::Permissions, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc};

use anyhow::Context;
use num_format::{Locale, ToFormattedString};
use rsnano_ledger::{Ledger, LedgerBuilder};
use rsnano_network_protocol::SynCookies;
use rsnano_nullable_clock::SteadyClock;
use rsnano_nullable_fs::NullableFilesystem;
use rsnano_nullable_lmdb::LmdbEnvironmentFactory;
use rsnano_types::{Networks, NodeId, PrivateKey};
use rsnano_utils::{
    container_info::ContainerInfoFactory,
    stats::Stats,
    sync::backpressure_channel::{self, Receiver, Sender},
    thread_pool::ThreadPool,
    ticker::TickerPool,
};
use tracing::info;

use crate::{
    block_processing::LedgerEvent,
    config::{GlobalConfig, NetworkParams, NodeConfig, NodeFlags},
    consensus::{get_bootstrap_weights, log_bootstrap_weights},
    node_id_key_file::NodeIdKeyFile,
    telemetry::{rsnano_build_info, rsnano_version_string},
    tokio_runner::TokioRunner,
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
    pub(crate) lmdb_env_factory: LmdbEnvironmentFactory,
    pub(crate) syn_cookies: Arc<SynCookies>,
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
        lmdb_env_factory,
        syn_cookies,
    })
}
