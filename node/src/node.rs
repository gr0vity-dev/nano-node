use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender},
    },
    time::Duration,
};

use tracing::{error, info};

use rsnano_ledger::{AnySet, BlockError, LedgerSet};
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
    ticker::{TickerPool, TimerThread},
};

#[cfg(feature = "ledger_snapshots")]
use crate::ledger_snapshots::LedgerSnapshots;
use crate::{
    BootstrapWorkServices, ConsensusServices, LedgerQueryServices, NetworkServices, NodeCallbacks,
    NodeServices, TelemetryServices, WalletServices,
    block_processing::{BacklogScan, BlockContext, BlockSource, ProcessedResult, UncheckedMap},
    config::{NetworkParams, NodeConfig, NodeFlags},
    consensus::{
        AecTicker, AecVoter, VoteCacheProcessor, VoteRebroadcaster, election::ConfirmedElection,
    },
    node_builder::NodeParts,
    node_id_key_file::NodeIdKeyFile,
    tokio_runner::TokioRunner,
};

#[allow(dead_code)]
pub struct Node {
    is_nulled: bool,
    pub runtime: tokio::runtime::Handle,
    pub data_path: PathBuf,
    pub node_id: PrivateKey,
    pub config: NodeConfig,
    pub network_params: NetworkParams,
    workers: Arc<ThreadPool>,
    pub flags: NodeFlags,
    services: NodeServices,
    pub unchecked: Arc<Mutex<UncheckedMap>>,
    pub backlog_scan: BacklogScan,
    vote_cache_processor: Arc<VoteCacheProcessor>,
    stopped: AtomicBool,
    start_stop_listener: OutputListenerMt<&'static str>,
    vote_rebroadcaster: VoteRebroadcaster,
    tokio_runner: TokioRunner,
    pub aec_ticker: TimerThread<AecTicker>,
    pub stats_collector: StatsCollector,
    container_info_factory: ContainerInfoFactory,
    aec_voter: TimerThread<AecVoter>,
    ticker_pool: TickerPool,
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
        Self::new(args, true, NodeIdKeyFile::new_null())
    }

    pub(crate) fn new_with_args(args: NodeArgs) -> Self {
        Self::new(args, false, NodeIdKeyFile::default())
    }

    pub fn node_id(&self) -> NodeId {
        self.node_id.public_key().into()
    }

    pub fn services(&self) -> &NodeServices {
        &self.services
    }

    pub fn wallet_services(&self) -> WalletServices {
        self.services.wallet_services()
    }

    pub fn telemetry_services(&self) -> TelemetryServices {
        self.services.telemetry_services()
    }

    pub fn network_services(&self) -> NetworkServices {
        self.services.network_services()
    }

    pub fn consensus_services(&self) -> ConsensusServices {
        self.services.consensus_services()
    }

    pub fn ledger_query_services(&self) -> LedgerQueryServices {
        self.services.ledger_query_services()
    }

    pub fn bootstrap_work_services(&self) -> BootstrapWorkServices {
        self.services.bootstrap_work_services()
    }

    pub fn stats_service(&self) -> Arc<Stats> {
        self.services.stats.clone()
    }

    fn new(args: NodeArgs, is_nulled: bool, node_id_key_file: NodeIdKeyFile) -> Self {
        let parts = crate::node_builder::build_node_parts(args, is_nulled, node_id_key_file);
        Self::from_parts(parts)
    }

    fn from_parts(parts: NodeParts) -> Self {
        Self {
            is_nulled: parts.is_nulled,
            runtime: parts.runtime,
            data_path: parts.data_path,
            node_id: parts.node_id,
            config: parts.config,
            network_params: parts.network_params,
            workers: parts.workers,
            flags: parts.flags,
            services: parts.services,
            unchecked: parts.unchecked,
            backlog_scan: parts.backlog_scan,
            vote_cache_processor: parts.vote_cache_processor,
            stopped: AtomicBool::new(false),
            start_stop_listener: OutputListenerMt::new(),
            vote_rebroadcaster: parts.vote_rebroadcaster,
            tokio_runner: parts.tokio_runner,
            aec_ticker: parts.aec_ticker,
            stats_collector: parts.stats_collector,
            container_info_factory: parts.container_info_factory,
            aec_voter: parts.aec_voter,
            ticker_pool: parts.ticker_pool,
            #[cfg(feature = "ledger_snapshots")]
            ledger_snapshots: parts.ledger_snapshots,
        }
    }

    pub fn container_info(&self) -> ContainerInfo {
        self.container_info_factory.container_info()
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    pub fn process_local(&self, block: Block) -> Result<(), BlockError> {
        self.consensus_services()
            .block_processor_queue
            .push_blocking(Arc::new(block), BlockSource::Local)
            .map_err(|_| BlockError::BadSignature)?
            .map(|_| {})
    }

    pub fn try_process(&self, block: Block) -> Result<SavedBlock, BlockError> {
        self.ledger_query_services().ledger.process_one(&block)
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
            match self.ledger_query_services().ledger.process_one(block) {
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
        self.consensus_services()
            .block_processor_queue
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
        self.ledger_query_services().ledger.any().get_block(hash)
    }

    pub fn latest(&self, account: &Account) -> BlockHash {
        self.ledger_query_services()
            .ledger
            .any()
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
        self.ledger_query_services().ledger.any().block_exists(hash)
    }

    pub fn blocks_exist(&self, hashes: &[Block]) -> bool {
        self.block_hashes_exist(hashes.iter().map(|b| b.hash()))
    }

    pub fn block_hashes_exist(&self, hashes: impl IntoIterator<Item = BlockHash>) -> bool {
        let ledger_services = self.ledger_query_services();
        let any = ledger_services.ledger.any();
        hashes.into_iter().all(|h| any.block_exists(&h))
    }

    pub fn balance(&self, account: &Account) -> Amount {
        let ledger_services = self.ledger_query_services();
        ledger_services.ledger.any().account_balance(account)
    }

    pub fn confirm_multi(&self, blocks: &[Block]) {
        for block in blocks {
            self.confirm(block.hash());
        }
    }

    pub fn confirm(&self, hash: BlockHash) {
        self.ledger_query_services().ledger.confirm(hash);
    }

    pub fn block_confirmed(&self, hash: &BlockHash) -> bool {
        self.ledger_query_services()
            .ledger
            .confirmed()
            .block_exists(hash)
    }

    pub fn block_hashes_confirmed(&self, blocks: &[BlockHash]) -> bool {
        let ledger_services = self.ledger_query_services();
        let confirmed = ledger_services.ledger.confirmed();
        blocks.iter().all(|b| confirmed.block_exists(b))
    }

    pub fn blocks_confirmed(&self, blocks: &[Block]) -> bool {
        let ledger_services = self.ledger_query_services();
        let confirmed = ledger_services.ledger.confirmed();
        blocks.iter().all(|b| confirmed.block_exists(&b.hash()))
    }

    pub fn is_active_root(&self, root: &QualifiedRoot) -> bool {
        self.consensus_services()
            .active
            .read()
            .unwrap()
            .is_active_root(root)
    }

    pub fn is_active_hash(&self, hash: &BlockHash) -> bool {
        self.consensus_services()
            .active
            .read()
            .unwrap()
            .is_active_hash(hash)
    }

    pub fn force_confirm(&self, hash: &BlockHash) {
        assert_eq!(
            self.network_params.network.current_network,
            Networks::NanoDevNetwork
        );
        let now = self.network_services().steady_clock.now();
        self.consensus_services()
            .active
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
            .services
            .ledger
            .any()
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

        let network_services = self.network_services();
        let consensus_services = self.consensus_services();
        let bootstrap_work_services = self.bootstrap_work_services();
        let telemetry_services = self.telemetry_services();

        network_services.start(self.config.tcp.max_inbound_connections);
        self.aec_voter.start(Duration::from_millis(20));

        consensus_services.start(&self.config, &self.flags);
        self.vote_cache_processor.start();
        if !self.flags.disable_request_loop {
            self.aec_ticker
                .start(self.network_params.network.aec_loop_interval);
        }
        self.backlog_scan.start();
        bootstrap_work_services.start(self.config.enable_bootstrap_responder);
        telemetry_services.start();

        if self.config.enable_vote_rebroadcast {
            self.vote_rebroadcaster.start();
        }
        self.ticker_pool.start();
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

        let network_services = self.network_services();
        let consensus_services = self.consensus_services();
        let bootstrap_work_services = self.bootstrap_work_services();
        let telemetry_services = self.telemetry_services();
        let wallet_services = self.wallet_services();

        self.ticker_pool.stop();
        network_services.stop_listeners();
        self.aec_voter.stop();
        bootstrap_work_services.stop();
        self.backlog_scan.stop();
        self.vote_cache_processor.stop();
        self.aec_ticker.stop();
        consensus_services.stop();
        telemetry_services.stop();
        wallet_services.stop();
        network_services.stop_threads(); // Stop network last to avoid killing in-use sockets
        self.vote_rebroadcaster.stop();
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
    use std::any::type_name;

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
            let Some(interval) = node.ticker_pool.get::<T>() else {
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
        let node = Node::new(args, true, NodeIdKeyFile::new_null());
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
        let consensus_services = node.consensus_services();
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
