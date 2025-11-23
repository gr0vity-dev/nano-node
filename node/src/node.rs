use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender},
    },
};

use rsnano_ledger::BlockError;
use rsnano_network::ChannelId;
#[cfg(any(test, feature = "test_support"))]
use rsnano_nullable_clock::Timestamp;
use rsnano_output_tracker::OutputListenerMt;
use rsnano_types::{
    Account, Amount, Block, BlockHash, Networks, NodeId, PrivateKey, QualifiedRoot, Root,
    SavedBlock, Vote, VoteError, WorkNonce, WorkRequest,
};
#[cfg(any(test, feature = "test_support"))]
use rsnano_utils::stats::{DetailType, StatType};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoFactory, ContainerInfoProvider},
    stats::{Direction, Stats, StatsCollection, StatsCollector},
};

#[cfg(feature = "ledger_snapshots")]
use crate::ledger_snapshots::LedgerSnapshots;
use crate::{
    BacklogServices, BootstrapWorkServices, LedgerQueryServices, NodeCallbacks, ProductionHandles,
    WalletServices,
    block_processing::{BlockContext, BlockSource, ProcessedResult, UncheckedHandle, UncheckedMap},
    config::{NetworkParams, NodeConfig, NodeFlags},
    consensus::election::ConfirmedElection,
    node_builder::{ComposedNode, NodeBuildError, NodeBuildResult},
    node_id_key_file::NodeIdKeyFile,
    subsystems::{
        BacklogSubsystem, BootstrapSubsystem, BootstrapWiring, ConsensusContext,
        ConsensusSubsystem, ConsensusWiring, Lifecycle, NetworkSubsystem, NetworkWiring,
        TelemetrySubsystem, TelemetryWiring, TickerSubsystem, WalletSubsystem,
    },
    tokio_runner::TokioRunner,
};
#[cfg(test)]
use crate::{TelemetryServices, consensus::AecTicker};
#[cfg(test)]
use rsnano_utils::ticker::TimerThread;

#[allow(dead_code)]
pub struct Node {
    // private: callers must use runtime() accessor
    runtime: tokio::runtime::Handle,
    data_path: PathBuf,
    node_id: PrivateKey,
    config: NodeConfig,
    network_params: NetworkParams,
    flags: NodeFlags,
    wallet_subsystem: WalletSubsystem,
    ledger_query_services: LedgerQueryServices,
    stats: Arc<Stats>,
    handles: ProductionHandles,
    network_subsystem: NetworkSubsystem,
    consensus_subsystem: ConsensusSubsystem,
    backlog_subsystem: BacklogSubsystem,
    unchecked: Arc<Mutex<UncheckedMap>>,
    stopped: AtomicBool,
    start_stop_listener: OutputListenerMt<&'static str>,
    tokio_runner: TokioRunner,
    stats_collector: StatsCollector,
    container_info_factory: ContainerInfoFactory,
    ticker_subsystem: TickerSubsystem,
    bootstrap_subsystem: BootstrapSubsystem,
    telemetry_subsystem: TelemetrySubsystem,
    #[cfg(feature = "ledger_snapshots")]
    ledger_snapshots: Arc<LedgerSnapshots>,
    #[cfg_attr(not(any(test, feature = "test_support")), allow(dead_code))]
    bootstrap_work_services: BootstrapWorkServices,
}

#[cfg(any(test, feature = "test_support"))]
#[derive(Clone)]
pub struct StatsHandle {
    stats: Arc<Stats>,
}

#[cfg(any(test, feature = "test_support"))]
impl StatsHandle {
    pub fn count(&self, stat: StatType, detail: DetailType, dir: Direction) -> u64 {
        self.stats.count(stat, detail, dir)
    }
}

#[cfg(any(test, feature = "test_support"))]
impl std::ops::Deref for StatsHandle {
    type Target = Stats;

    fn deref(&self) -> &Self::Target {
        &self.stats
    }
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
    /// Creates a node with no-op subsystems suitable for tests; start/stop are safe to call.
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

    pub(crate) fn new_with_args(args: NodeArgs) -> NodeBuildResult<Self> {
        Self::build_from_args(args, false, NodeIdKeyFile::default())
    }

    pub fn node_id(&self) -> NodeId {
        self.node_id.public_key().into()
    }

    pub fn wallet_services(&self) -> WalletServices {
        self.wallet_subsystem.services()
    }

    pub fn telemetry_subsystem(&self) -> TelemetrySubsystem {
        self.telemetry_subsystem.clone()
    }

    pub fn runtime(&self) -> tokio::runtime::Handle {
        self.runtime.clone()
    }

    pub fn data_path(&self) -> &PathBuf {
        &self.data_path
    }

    pub fn config(&self) -> &NodeConfig {
        &self.config
    }

    pub fn network_params(&self) -> &NetworkParams {
        &self.network_params
    }

    pub fn flags(&self) -> &NodeFlags {
        &self.flags
    }

    #[cfg(test)]
    pub fn telemetry_services(&self) -> TelemetryServices {
        self.telemetry_subsystem.telemetry_services()
    }

    pub fn network_subsystem(&self) -> NetworkSubsystem {
        self.network_subsystem.clone()
    }

    pub fn consensus_subsystem(&self) -> ConsensusSubsystem {
        self.consensus_subsystem.clone()
    }

    pub fn production_handles(&self) -> ProductionHandles {
        self.handles.clone()
    }

    pub fn ledger_query_services(&self) -> LedgerQueryServices {
        self.ledger_query_services.clone()
    }

    #[cfg(any(test, feature = "test_support"))]
    #[doc(hidden)]
    pub fn bootstrap_work_services(&self) -> BootstrapWorkServices {
        self.bootstrap_work_services.clone()
    }

    pub fn bootstrap_subsystem(&self) -> BootstrapSubsystem {
        self.bootstrap_subsystem.clone()
    }

    pub fn stats_snapshot(&self) -> MutexGuard<'_, StatsCollection> {
        self.stats_collector.collect()
    }

    pub fn stats_count(&self, stat: &'static str, detail: &'static str, dir: Direction) -> u64 {
        self.stats_snapshot().get_dir(stat, detail, dir)
    }

    #[cfg(any(test, feature = "test_support"))]
    #[doc(hidden)]
    pub fn stats_service(&self) -> StatsHandle {
        StatsHandle {
            stats: self.stats.clone(),
        }
    }

    pub fn ticker_subsystem(&self) -> &TickerSubsystem {
        &self.ticker_subsystem
    }

    pub fn unchecked(&self) -> UncheckedHandle {
        UncheckedHandle::new(self.unchecked.clone())
    }

    /// Submit a block to the unchecked map using the current timestamp.
    pub fn submit_unchecked(&self, dependency: BlockHash, block: Block) {
        let now = self.network_subsystem.now();
        self.unchecked.lock().unwrap().put(dependency, block, now);
    }

    /// Test-only helper to submit with an explicit timestamp.
    #[cfg(any(test, feature = "test_support"))]
    pub fn submit_unchecked_at(&self, dependency: BlockHash, block: Block, now: Timestamp) {
        self.unchecked.lock().unwrap().put(dependency, block, now);
    }

    pub fn stats_collector(&self) -> &StatsCollector {
        &self.stats_collector
    }

    pub fn backlog_scan(&self) -> &BacklogServices {
        self.backlog_subsystem.services()
    }

    #[cfg(test)]
    pub(crate) fn aec_ticker(&self) -> Arc<TimerThread<AecTicker>> {
        self.consensus_subsystem.aec_ticker()
    }

    #[cfg(all(test, feature = "ledger_snapshots"))]
    #[doc(hidden)]
    pub(crate) fn ledger_snapshots(&self) -> &LedgerSnapshots {
        &self.ledger_snapshots
    }

    pub fn spawn_blocking<F, R>(&self, f: F) -> tokio::task::JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.runtime.spawn_blocking(f)
    }

    fn build_from_args(
        args: NodeArgs,
        is_nulled: bool,
        node_id_key_file: NodeIdKeyFile,
    ) -> NodeBuildResult<Self> {
        let composed = crate::node_builder::compose_root(args, is_nulled, node_id_key_file)?;
        Self::validate_genesis_block(&composed)?;
        Self::new(composed).map_err(NodeBuildError::from)
    }

    fn validate_genesis_block(composed: &ComposedNode) -> Result<(), NodeBuildError> {
        let genesis_hash = composed.network_params.ledger.genesis_block.hash();
        if composed
            .ledger_query_services
            .ledger_queries()
            .block_exists(&genesis_hash)
        {
            Ok(())
        } else {
            Err(NodeBuildError::GenesisBlockMissing {
                data_path: composed.data_path.clone(),
                network: composed.network_params.network.current_network,
                genesis_hash,
            })
        }
    }

    pub(crate) fn new(composed: ComposedNode) -> anyhow::Result<Self> {
        let max_inbound_connections = composed.config.tcp.max_inbound_connections;
        let network_subsystem = {
            let wiring = NetworkWiring {
                network: composed.network.clone(),
                tcp_listener: composed.tcp_listener.clone(),
                peer_connector: composed.peer_connector.clone(),
                network_threads: composed.network_threads.clone(),
                message_processor: composed.message_processor.clone(),
                message_sender: composed.message_sender.clone(),
                message_flooder: composed.message_flooder.clone(),
                keepalive_publisher: composed.keepalive_publisher.clone(),
                inbound_message_queue: composed.inbound_message_queue.clone(),
                network_filter: composed.network_filter.clone(),
                steady_clock: composed.steady_clock.clone(),
            };
            NetworkSubsystem::new(wiring, composed.workers.clone(), max_inbound_connections)
        };

        let consensus_subsystem = {
            let wiring = ConsensusWiring {
                active: composed.active.clone(),
                election_schedulers: composed.election_schedulers.clone(),
                vote_processor: composed.vote_processor.clone(),
                vote_generators: composed.vote_generators.clone(),
                vote_history: composed.vote_history.clone(),
                request_aggregator: composed.request_aggregator.clone(),
                bounded_backlog: composed.bounded_backlog.clone(),
                bootstrapper: composed.bootstrapper.clone(),
                rep_crawler: composed.rep_crawler.clone(),
                online_reps: composed.online_reps.clone(),
                rep_tiers: composed.rep_tiers.clone(),
                local_block_broadcaster: composed.local_block_broadcaster.clone(),
                winner_block_broadcaster: composed.winner_block_broadcaster.clone(),
                vote_processor_queue: composed.vote_processor_queue.clone(),
                vote_cache: composed.vote_cache.clone(),
                vote_cache_processor: composed.vote_cache_processor.clone(),
                confirming_set: composed.confirming_set.clone(),
                block_processor: composed.block_processor.clone(),
                block_processor_queue: composed.block_processor_queue.clone(),
                vote_rebroadcaster: composed.vote_rebroadcaster.clone(),
            };
            let context = ConsensusContext {
                config: composed.config.clone(),
                flags: composed.flags.clone(),
                network_params: composed.network_params.clone(),
                aec_ticker: composed.aec_ticker.clone(),
                aec_voter: composed.aec_voter.clone(),
            };
            ConsensusSubsystem::new(wiring, context)
        };
        let bootstrap_wiring = BootstrapWiring::new(
            composed.bootstrapper.clone(),
            composed.bootstrap_server.clone(),
            composed.work_factory.clone(),
        );
        let bootstrap_subsystem =
            BootstrapSubsystem::new(bootstrap_wiring, composed.config.enable_bootstrap_responder);
        let telemetry_wiring =
            TelemetryWiring::new(composed.telemetry.clone(), composed.tcp_listener.clone());
        let telemetry_subsystem = TelemetrySubsystem::new(telemetry_wiring);
        let ticker_subsystem = TickerSubsystem::new(composed.ticker_services);
        let handles = ProductionHandles::new(composed.ledger.clone());
        let wallet_subsystem = WalletSubsystem::new(composed.wallet_services.clone());
        let ledger_query_services = composed.ledger_query_services.clone();
        let bootstrap_work_services = composed.bootstrap_work_services.clone();
        let stats = composed.stats.clone();
        let backlog_subsystem = BacklogSubsystem::new(composed.backlog_scan);

        Ok(Self {
            runtime: composed.runtime,
            data_path: composed.data_path,
            node_id: composed.node_id,
            config: composed.config,
            network_params: composed.network_params,
            flags: composed.flags,
            wallet_subsystem,
            ledger_query_services,
            bootstrap_work_services,
            stats,
            handles,
            network_subsystem,
            consensus_subsystem,
            bootstrap_subsystem,
            telemetry_subsystem,
            ticker_subsystem,
            unchecked: composed.unchecked,
            backlog_subsystem,
            stopped: AtomicBool::new(false),
            start_stop_listener: OutputListenerMt::new(),
            tokio_runner: composed.tokio_runner,
            stats_collector: composed.stats_collector,
            container_info_factory: composed.container_info_factory,
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
            .push_block_blocking(block, BlockSource::Local)
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
        self.consensus_subsystem.enqueue_block(BlockContext::new(
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

    #[cfg(any(test, feature = "test_support"))]
    #[doc(hidden)]
    pub fn work_generate_dev(&self, root: impl Into<Root>) -> WorkNonce {
        let difficulty = self.network_params.work.threshold_base();
        self.bootstrap_work_services()
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
        self.consensus_subsystem.is_active_root(root)
    }

    pub fn is_active_hash(&self, hash: &BlockHash) -> bool {
        self.consensus_subsystem.is_active_hash(hash)
    }

    pub fn force_confirm(&self, hash: &BlockHash) {
        assert_eq!(
            self.network_params.network.current_network,
            Networks::NanoDevNetwork
        );
        let now = self.network_subsystem.now();
        self.consensus_subsystem.force_confirm(hash, now);
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
        self.network_subsystem.start();
        self.consensus_subsystem.start();
        self.backlog_subsystem.start();
        self.bootstrap_subsystem.start();
        self.telemetry_subsystem.start();
        self.ticker_subsystem.start();
        self.wallet_subsystem.start();
    }

    pub fn stop(&mut self) {
        self.start_stop_listener.emit("stop");
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        self.wallet_subsystem.stop();
        self.ticker_subsystem.stop();
        self.telemetry_subsystem.stop();
        self.bootstrap_subsystem.stop();
        self.backlog_subsystem.stop();
        self.consensus_subsystem.stop();
        self.network_subsystem.stop();
        self.tokio_runner.stop();
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
            let Some(interval) = node.ticker_subsystem().interval_for::<T>() else {
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
        let ticker_handle = node.aec_ticker();
        let task = ticker_handle.task();
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
