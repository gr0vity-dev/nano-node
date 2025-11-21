use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender},
    },
};

use tracing::{error, info};

use rsnano_ledger::BlockError;
use rsnano_network::ChannelId;
use rsnano_output_tracker::OutputListenerMt;
use rsnano_types::{
    Account, Amount, Block, BlockHash, Networks, NodeId, PrivateKey, QualifiedRoot, Root,
    SavedBlock, Vote, VoteError, WorkNonce, WorkRequest,
};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoFactory, ContainerInfoProvider},
    stats::{Direction, Stats, StatsCollection, StatsCollector},
    thread_pool::ThreadPool,
    ticker::TimerThread,
};

#[cfg(test)]
use crate::TelemetryServices;
#[cfg(feature = "ledger_snapshots")]
use crate::ledger_snapshots::LedgerSnapshots;
use crate::{
    BacklogServices, BootstrapWorkServices, ConsensusServices, ConsensusTimerServices,
    LedgerQueryServices, NodeCallbacks, NodeServices, ProductionHandles, WalletServices,
    block_processing::{BlockContext, BlockSource, ProcessedResult, UncheckedMap},
    config::{NetworkParams, NodeConfig, NodeFlags},
    consensus::{AecTicker, AecVoter, election::ConfirmedElection},
    node_builder::ComposedNode,
    node_id_key_file::NodeIdKeyFile,
    subsystems::{
        BootstrapSubsystem, ConsensusSubsystem, Lifecycle, NetworkSubsystem, TelemetrySubsystem,
        TickerSubsystem,
    },
    tokio_runner::TokioRunner,
};

#[allow(dead_code)]
pub struct Node {
    is_nulled: bool,
    runtime: tokio::runtime::Handle,
    pub data_path: PathBuf,
    pub node_id: PrivateKey,
    pub config: NodeConfig,
    pub network_params: NetworkParams,
    workers: Arc<ThreadPool>,
    pub flags: NodeFlags,
    services: NodeServices,
    handles: ProductionHandles,
    network_subsystem: NetworkSubsystem,
    consensus_subsystem: ConsensusSubsystem,
    unchecked: Arc<Mutex<UncheckedMap>>,
    pub backlog_scan: BacklogServices,
    stopped: AtomicBool,
    start_stop_listener: OutputListenerMt<&'static str>,
    tokio_runner: TokioRunner,
    pub aec_ticker: TimerThread<AecTicker>,
    pub stats_collector: StatsCollector,
    container_info_factory: ContainerInfoFactory,
    aec_voter: TimerThread<AecVoter>,
    ticker_subsystem: TickerSubsystem,
    bootstrap_subsystem: BootstrapSubsystem,
    telemetry_subsystem: TelemetrySubsystem,
    #[cfg(feature = "ledger_snapshots")]
    pub ledger_snapshots: Arc<LedgerSnapshots>,
}

pub(crate) struct NodeArgs {
    pub data_path: PathBuf,
    pub config: NodeConfig,
    pub network_params: NetworkParams,
    pub flags: NodeFlags,
    pub callbacks: NodeCallbacks,
    pub event_sender: Option<SyncSender<NodeEvent>>,
}

impl NodeArgs {
    pub fn create_test_instance() -> Self {
        let network_params = NetworkParams::new(Networks::NanoLiveNetwork);
        let config = NodeConfig::new(None, &network_params, 2);
        Self {
            data_path: "/home/nulled-node".into(),
            network_params,
            config,
            flags: Default::default(),
            callbacks: Default::default(),
            event_sender: None,
        }
    }
}

impl Node {
    pub fn new_null() -> Self {
        Self::new_null_with_callbacks(Default::default())
    }

    pub fn new_null_with_callbacks(callbacks: NodeCallbacks) -> Self {
        let args = NodeArgs {
            callbacks,
            ..NodeArgs::create_test_instance()
        };
        Self::build_from_args(args, true, NodeIdKeyFile::new_null())
            .expect("null node initialization failed")
    }

    pub(crate) fn new_with_args(args: NodeArgs) -> anyhow::Result<Self> {
        Self::build_from_args(args, false, NodeIdKeyFile::default())
    }

    pub fn node_id(&self) -> NodeId {
        self.node_id.public_key().into()
    }

    pub fn wallet_services(&self) -> WalletServices {
        self.services.wallet_services()
    }

    pub fn telemetry_subsystem(&self) -> TelemetrySubsystem {
        self.telemetry_subsystem.clone()
    }

    pub fn runtime(&self) -> tokio::runtime::Handle {
        self.runtime.clone()
    }

    #[cfg(test)]
    pub fn telemetry_services(&self) -> TelemetryServices {
        self.services.telemetry_services()
    }

    pub fn network_subsystem(&self) -> NetworkSubsystem {
        self.network_subsystem.clone()
    }

    pub fn consensus_subsystem(&self) -> ConsensusSubsystem {
        self.consensus_subsystem.clone()
    }

    pub(crate) fn services(&self) -> &NodeServices {
        &self.services
    }

    pub fn production_handles(&self) -> ProductionHandles {
        self.handles.clone()
    }

    pub fn ledger_query_services(&self) -> LedgerQueryServices {
        self.services.ledger_query_services()
    }

    pub fn bootstrap_work_services(&self) -> BootstrapWorkServices {
        self.services.bootstrap_work_services()
    }

    pub fn bootstrap_subsystem(&self) -> BootstrapSubsystem {
        self.bootstrap_subsystem.clone()
    }

    pub fn stats_service(&self) -> Arc<Stats> {
        self.services.stats.clone()
    }

    pub fn ticker_subsystem(&self) -> &TickerSubsystem {
        &self.ticker_subsystem
    }

    fn consensus_timer_services(&self) -> ConsensusTimerServices<'_> {
        ConsensusTimerServices::new(&self.aec_ticker, &self.aec_voter)
    }

    pub fn unchecked(&self) -> Arc<Mutex<UncheckedMap>> {
        self.unchecked.clone()
    }

    pub fn stats_collector(&self) -> StatsCollector {
        self.stats_collector.clone()
    }

    fn build_from_args(
        args: NodeArgs,
        is_nulled: bool,
        node_id_key_file: NodeIdKeyFile,
    ) -> anyhow::Result<Self> {
        let composed = crate::node_builder::compose_root(args, is_nulled, node_id_key_file)?;
        Self::new(composed)
    }

    pub(crate) fn new(composed: ComposedNode) -> anyhow::Result<Self> {
        let max_inbound_connections = composed.config.tcp.max_inbound_connections;
        let network_subsystem = {
            let services = &composed.services;
            NetworkSubsystem::new(
                services.network.clone(),
                services.tcp_listener.clone(),
                services.peer_connector.clone(),
                services.network_threads.clone(),
                services.message_processor.clone(),
                services.message_sender.clone(),
                services.message_flooder.clone(),
                services.keepalive_publisher.clone(),
                services.inbound_message_queue.clone(),
                services.network_filter.clone(),
                services.steady_clock.clone(),
                max_inbound_connections,
            )
        };

        let consensus_subsystem = {
            let s = &composed.services;
            let services = ConsensusServices::new(
                s.active.clone(),
                s.election_schedulers.clone(),
                s.vote_processor.clone(),
                s.vote_generators.clone(),
                s.vote_history.clone(),
                s.request_aggregator.clone(),
                s.bounded_backlog.clone(),
                s.bootstrapper.clone(),
                s.rep_crawler.clone(),
                s.online_reps.clone(),
                s.rep_tiers.clone(),
                s.local_block_broadcaster.clone(),
                s.winner_block_broadcaster.clone(),
                s.vote_processor_queue.clone(),
                s.vote_cache.clone(),
                s.vote_cache_processor.clone(),
                s.confirming_set.clone(),
                s.block_processor.clone(),
                s.block_processor_queue.clone(),
                s.vote_rebroadcaster.clone(),
            );
            ConsensusSubsystem::new(services, composed.config.clone(), composed.flags.clone())
        };
        let bootstrap_subsystem = BootstrapSubsystem::new(
            composed.services.bootstrapper.clone(),
            composed.services.bootstrap_server.clone(),
            composed.services.work_factory.clone(),
            composed.config.enable_bootstrap_responder,
        );
        let telemetry_subsystem = TelemetrySubsystem::new(
            composed.services.telemetry.clone(),
            composed.services.tcp_listener.clone(),
        );
        let ticker_subsystem = TickerSubsystem::new(composed.ticker_services);
        let handles = ProductionHandles::new(composed.services.ledger());

        Ok(Self {
            is_nulled: composed.is_nulled,
            runtime: composed.runtime,
            data_path: composed.data_path,
            node_id: composed.node_id,
            config: composed.config,
            network_params: composed.network_params,
            workers: composed.workers,
            flags: composed.flags,
            services: composed.services,
            handles,
            network_subsystem,
            consensus_subsystem,
            bootstrap_subsystem,
            telemetry_subsystem,
            ticker_subsystem,
            unchecked: composed.unchecked,
            backlog_scan: composed.backlog_scan,
            stopped: AtomicBool::new(false),
            start_stop_listener: OutputListenerMt::new(),
            tokio_runner: composed.tokio_runner,
            aec_ticker: composed.aec_ticker,
            stats_collector: composed.stats_collector,
            container_info_factory: composed.container_info_factory,
            aec_voter: composed.aec_voter,
            #[cfg(feature = "ledger_snapshots")]
            ledger_snapshots: composed.ledger_snapshots,
        })
    }

    pub fn container_info(&self) -> ContainerInfo {
        self.container_info_factory.container_info()
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    pub fn process_local(&self, block: Block) -> Result<(), BlockError> {
        self.consensus_subsystem
            .block_processor_queue()
            .push_blocking(Arc::new(block), BlockSource::Local)
            .map_err(|_| BlockError::BadSignature)?
            .map(|_| {})
    }

    pub fn try_process(&self, block: Block) -> Result<SavedBlock, BlockError> {
        self.production_handles()
            .ledger_queries()
            .process_one(&block)
    }

    pub fn process(&self, block: Block) -> SavedBlock {
        let hash = block.hash();
        match self.try_process(block) {
            Ok(saved_block) => saved_block,
            Err(BlockError::Old) | Err(BlockError::Conflict) => self.block(&hash).unwrap(),
            Err(e) => {
                panic!("Could not process block: {:?}", e);
            }
        }
    }

    pub fn process_multi(&self, blocks: &[Block]) {
        for (i, block) in blocks.iter().enumerate() {
            match self
                .production_handles()
                .ledger_queries()
                .process_one(block)
            {
                Ok(_) | Err(BlockError::Old) | Err(BlockError::Conflict) => {}
                Err(e) => {
                    panic!("Could not multi-process block index {}: {:?}", i, e);
                }
            }
        }
    }

    pub fn process_and_confirm_multi(&self, blocks: &[Block]) {
        self.process_multi(blocks);
        self.confirm_multi(blocks);
    }

    pub fn process_active(&self, block: Block) {
        self.consensus_subsystem
            .block_processor_queue()
            .push(BlockContext::new(
                block,
                BlockSource::Live,
                ChannelId::LOOPBACK,
            ));
    }

    pub fn process_local_multi(&self, blocks: &[Block]) {
        for block in blocks {
            let status = self.process_local(block.clone());
            if !matches!(status, Ok(()) | Err(BlockError::Old)) {
                panic!("could not process block!");
            }
        }
    }

    pub fn block(&self, hash: &BlockHash) -> Option<SavedBlock> {
        self.production_handles().ledger_queries().get_block(hash)
    }

    pub fn latest(&self, account: &Account) -> BlockHash {
        self.production_handles()
            .ledger_queries()
            .account_head(account)
            .unwrap_or_default()
    }

    pub fn get_node_id(&self) -> NodeId {
        self.node_id.public_key().into()
    }

    pub fn work_generate_dev(&self, root: impl Into<Root>) -> WorkNonce {
        let difficulty = self.network_params.work.threshold_base();
        self.bootstrap_work_services()
            .work_factory
            .generate_work(WorkRequest::new(root.into(), difficulty))
            .unwrap()
    }

    pub fn block_exists(&self, hash: &BlockHash) -> bool {
        self.production_handles()
            .ledger_queries()
            .block_exists(hash)
    }

    pub fn blocks_exist(&self, hashes: &[Block]) -> bool {
        self.block_hashes_exist(hashes.iter().map(|b| b.hash()))
    }

    pub fn block_hashes_exist(&self, hashes: impl IntoIterator<Item = BlockHash>) -> bool {
        let queries = self.production_handles().ledger_queries();
        hashes.into_iter().all(|h| queries.block_exists(&h))
    }

    pub fn balance(&self, account: &Account) -> Amount {
        self.production_handles()
            .ledger_queries()
            .account_balance(account)
    }

    pub fn confirm_multi(&self, blocks: &[Block]) {
        for block in blocks {
            self.confirm(block.hash());
        }
    }

    pub fn confirm(&self, hash: BlockHash) {
        self.production_handles()
            .ledger_queries()
            .confirm_block(hash);
    }

    pub fn block_confirmed(&self, hash: &BlockHash) -> bool {
        self.production_handles()
            .ledger_queries()
            .confirmed_block_exists(hash)
    }

    pub fn block_hashes_confirmed(&self, blocks: &[BlockHash]) -> bool {
        let queries = self.production_handles().ledger_queries();
        blocks.iter().all(|b| queries.confirmed_block_exists(b))
    }

    pub fn blocks_confirmed(&self, blocks: &[Block]) -> bool {
        let queries = self.production_handles().ledger_queries();
        blocks
            .iter()
            .all(|b| queries.confirmed_block_exists(&b.hash()))
    }

    pub fn is_active_root(&self, root: &QualifiedRoot) -> bool {
        self.consensus_subsystem
            .active()
            .read()
            .unwrap()
            .is_active_root(root)
    }

    pub fn is_active_hash(&self, hash: &BlockHash) -> bool {
        self.consensus_subsystem
            .active()
            .read()
            .unwrap()
            .is_active_hash(hash)
    }

    pub fn force_confirm(&self, hash: &BlockHash) {
        assert_eq!(
            self.network_params.network.current_network,
            Networks::NanoDevNetwork
        );
        let now = self.network_subsystem.steady_clock().now();
        self.consensus_subsystem
            .active()
            .write()
            .unwrap()
            .force_confirm(hash, now);
    }

    pub fn get_stat(&self, stat: &'static str, detail: &'static str, dir: Direction) -> u64 {
        self.stats_collector.collect().get_dir(stat, detail, dir)
    }

    pub fn stats(&self) -> MutexGuard<'_, StatsCollection> {
        self.stats_collector.collect()
    }

    /// Note: Start must not be called from an async thread, because it blocks!
    pub fn start(&mut self) {
        self.start_stop_listener.emit("start");
        if self.is_nulled {
            return; // TODO better nullability implementation
        }

        if !self
            .production_handles()
            .ledger_queries()
            .block_exists(&self.network_params.ledger.genesis_block.hash())
        {
            error!(
                "Genesis block not found. This commonly indicates a configuration issue, check that the --network or --data_path command line arguments are correct, and also the ledger backend node config option. If using a read-only CLI command a ledger must already exist, start the node with --daemon first."
            );

            if self.network_params.network.is_beta_network() {
                error!("Beta network may have reset, try clearing database files");
            }

            panic!("Genesis block not found!");
        }

        let mut telemetry_services = self.telemetry_subsystem();

        self.network_subsystem.start();
        self.consensus_timer_services()
            .start(&self.flags, &self.network_params);

        Lifecycle::start(&mut self.consensus_subsystem);
        self.backlog_scan.start();
        Lifecycle::start(&mut self.bootstrap_subsystem);
        telemetry_services.start();

        self.ticker_subsystem.start();
    }

    pub fn stop(&mut self) {
        self.start_stop_listener.emit("stop");
        if self.is_nulled {
            return; // TODO better nullability implementation
        }

        // Ensure stop can only be called once
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        info!("Node stopping...");

        let mut telemetry_services = self.telemetry_subsystem();
        let wallet_services = self.wallet_services();

        self.ticker_subsystem.stop();
        self.network_subsystem.stop_listeners();
        self.consensus_timer_services().stop();
        Lifecycle::stop(&mut self.bootstrap_subsystem);
        self.backlog_scan.stop();
        Lifecycle::stop(&mut self.consensus_subsystem);
        telemetry_services.stop();
        wallet_services.stop();
        self.network_subsystem.stop_threads(); // Stop network last to avoid killing in-use sockets
        self.workers.join();
        self.tokio_runner.stop();
        // work pool is not stopped on purpose due to testing setup
    }
}

pub enum NodeEvent {
    ElectionStarted(BlockHash),
    ElectionStopped(BlockHash),
    BlockConfirmed(SavedBlock, ConfirmedElection),
    VoteProcessed(Arc<Vote>, Result<(), VoteError>),
    BlocksProcessed(Vec<ProcessedResult>),
}

pub trait NodeEventHandler {
    fn handle(&mut self, event: &NodeEvent);
}

pub struct CompositeNodeEventHandler {
    receiver: Receiver<NodeEvent>,
    handlers: Vec<Box<dyn NodeEventHandler + Send>>,
}
impl CompositeNodeEventHandler {
    pub fn new(receiver: Receiver<NodeEvent>) -> Self {
        Self {
            receiver,
            handlers: Vec::new(),
        }
    }

    pub fn add(&mut self, handler: impl NodeEventHandler + Send + 'static) {
        self.handlers.push(Box::new(handler));
    }

    pub fn run(&mut self) {
        while let Ok(event) = self.receiver.recv() {
            for handler in self.handlers.iter_mut() {
                handler.handle(&event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::{
        AecEvent, AecTickerPlugin, BootstrapStaleElections, ConfirmationSolicitorPlugin,
        RepTiersCalculator, StaleElectionsStats, WalletRepsChecker, WinnerBlockBroadcaster,
    };
    use crate::{
        block_processing::UncheckedBlockReenqueuer,
        block_rate_calculator::BlockRateCalculator,
        node_monitor::NodeMonitor,
        representatives::OnlineWeightCalculation,
        transport::{PeerCacheConnector, PeerCacheUpdater},
        wallets::LocalRepsComputation,
    };
    use rsnano_utils::{stats::StatsSource, ticker::Tickable};
    use rsnano_wallet::{ReceivableSearch, WalletBackup, WalletsTicker};
    use std::{any::type_name, time::Duration};

    #[test]
    fn schedule_tickers() {
        let node = Node::new_null();

        assert_ticker::<PeerCacheUpdater>(&node, Duration::from_secs(15));
        assert_ticker::<OnlineWeightCalculation>(&node, Duration::from_secs(20));
        assert_ticker::<RepTiersCalculator>(&node, Duration::from_secs(10));
        assert_ticker::<PeerCacheConnector>(&node, node.config.network.cached_peer_reachout);
        assert_ticker::<NodeMonitor>(&node, node.config.monitor.interval);
        assert_ticker::<WalletBackup>(&node, Duration::from_secs(60 * 5));
        assert_ticker::<ReceivableSearch>(&node, Duration::from_secs(5));
        assert_ticker::<WalletRepsChecker>(&node, Duration::from_secs(60));
        assert_ticker::<BlockRateCalculator>(&node, Duration::from_millis(500));
        assert_ticker::<UncheckedBlockReenqueuer>(&node, Duration::from_secs(1));
        assert_ticker::<LocalRepsComputation>(&node, Duration::from_secs(10));
        assert_ticker::<WalletsTicker>(&node, Duration::from_millis(500));

        // helper:
        fn assert_ticker<T: Tickable + 'static>(node: &Node, expected: Duration) {
            let Some(interval) = node.ticker_subsystem().ticker_pool().get::<T>() else {
                panic!("Should schedule ticker of type: {}", type_name::<T>());
            };
            assert_eq!(interval, expected, "interval for {}", type_name::<T>());
        }
    }

    #[test]
    fn initialize_aec_ticker() {
        let config = NodeConfig {
            bootstrap_stale_threshold: Duration::from_secs(42),
            ..NodeConfig::new_test_instance()
        };
        let args = NodeArgs {
            config: config.clone(),
            ..NodeArgs::create_test_instance()
        };
        let node = Node::build_from_args(args, true, NodeIdKeyFile::new_null())
            .expect("null node build failed");
        let task = node.aec_ticker.task();
        let ticker = task.as_ref().unwrap();

        assert_has_aec_ticker_plugin::<ConfirmationSolicitorPlugin>(ticker);

        let stale = assert_has_aec_ticker_plugin::<BootstrapStaleElections>(ticker);
        assert_eq!(
            stale.get_stale_threshold(),
            config.bootstrap_stale_threshold
        );
    }

    fn assert_has_aec_ticker_plugin<T>(ticker: &AecTicker) -> &T
    where
        T: AecTickerPlugin + 'static,
    {
        let plugin = ticker.get_plugin::<T>();
        assert!(
            plugin.is_some(),
            "AEC ticker plugin missing: {}",
            type_name::<T>()
        );
        plugin.unwrap()
    }

    #[test]
    fn initialize_stats_collector() {
        let node = Node::new_null();
        let node_stats = node.stats();
        assert_contains_stats_source(&node_stats, StaleElectionsStats::default());
        assert_contains_stats_source(&node_stats, WinnerBlockBroadcaster::new_null());
    }

    #[test]
    fn connect_winner_block_rebroadcaster() {
        let node = Node::new_null();
        let consensus_services = node.consensus_subsystem().test_handles();
        let broadcast_tracker = consensus_services
            .winner_block_broadcaster
            .lock()
            .unwrap()
            .track();
        let election = ConfirmedElection::new_test_instance();
        let winner_hash = election.winner.hash();

        consensus_services
            .active
            .write()
            .unwrap()
            .simulate_event(AecEvent::ElectionConfirmed(election));

        let output = broadcast_tracker.wait_output().unwrap();
        assert_eq!(output, vec![winner_hash]);
    }

    fn assert_contains_stats_source(node_stats: &StatsCollection, source: impl StatsSource) {
        let mut col = StatsCollection::default();
        source.collect_stats(&mut col);
        let (key, _) = col.iter().next().unwrap().clone();
        assert!(node_stats.contains(key.stat, key.detail, key.dir));
    }
}
