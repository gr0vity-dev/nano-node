use std::{
    ops::{Deref, DerefMut},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use bounded_vec_deque::BoundedVecDeque;

use crate::{
    block_processing::{
        BacklogScan, BlockProcessor, BlockProcessorQueue, BoundedBacklog, LocalBlockBroadcaster,
    },
    block_rate_calculator::CurrentBlockRates,
    bootstrap::{BootstrapExt, BootstrapServer, Bootstrapper},
    cementation::ConfirmingSet,
    config::{NetworkParams, NodeFlags},
    consensus::{
        ActiveElectionsContainer, AecTicker, AecVoter, CurrentRepTiers, LocalVoteHistory,
        RequestAggregator, VoteCache, VoteCacheProcessor, VoteGenerators, VoteProcessor,
        VoteProcessorQueue, VoteRebroadcaster, WinnerBlockBroadcaster,
        election::ConfirmedElection, election_schedulers::ElectionSchedulers,
    },
    representatives::{OnlineReps, RepCrawler},
    telemetry::{TelementryExt, Telemetry},
    transport::{
        MessageFlooder, MessageProcessor, MessageSender, NetworkThreads,
        keepalive::KeepalivePublisher,
    },
    handles::LedgerQueryHandle,
    wallets::WalletRepresentatives,
    work::WorkFactory,
};
use rsnano_ledger::Ledger;
use rsnano_messages::NetworkFilter;
use rsnano_network::{Network, PeerConnector, TcpListener};
use rsnano_network_protocol::InboundMessageQueue;
use rsnano_nullable_clock::SteadyClock;
use rsnano_store_lmdb::KeyType;
use rsnano_utils::{
    stats::Stats,
    ticker::{TickerPool, TimerThread},
};

#[cfg(feature = "ledger_snapshots")]
use crate::ledger_snapshots::LedgerSnapshots;

use rsnano_types::{Account, Amount, BlockHash, PrivateKey, PublicKey, RawKey, WalletId, WorkNonce};
use rsnano_wallet::{BlockPromise, MultiBlockPromise, Wallets, WalletsError};

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
    #[cfg(any(test, feature = "test_support"))]
    #[doc(hidden)]
    #[deprecated(
        note = "For tests/internal wiring only. Production code must use narrow wallet handles."
    )]
    pub wallets: Arc<Wallets>,
    #[cfg(not(any(test, feature = "test_support")))]
    wallets: Arc<Wallets>,
    pub work_factory: Arc<WorkFactory>,
    pub wallet_reps: Arc<Mutex<WalletRepresentatives>>,
}

#[allow(deprecated)]
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
        let wallet_id = self.wallet_ids()[0];
        self.insert_adhoc(&wallet_id, &keys.raw_key(), true)
            .unwrap();
    }

    pub fn stop(&self) {
        self.wallets.stop();
    }

    pub fn wallet_ids(&self) -> Vec<WalletId> {
        self.wallets.wallet_ids()
    }

    pub fn wallet_exists(&self, wallet_id: &WalletId) -> bool {
        self.wallets.wallet_exists(wallet_id)
    }

    pub fn create_wallet(&self, wallet_id: WalletId) {
        self.wallets.create(wallet_id);
    }

    pub fn destroy_wallet(&self, wallet_id: &WalletId) {
        self.wallets.destroy(wallet_id);
    }

    pub fn ensure_wallet_is_unlocked(&self, wallet_id: WalletId, password: &str) -> bool {
        self.wallets.ensure_wallet_is_unlocked(wallet_id, password)
    }

    pub fn rekey_wallet(&self, wallet_id: &WalletId, password: &str) -> Result<(), WalletsError> {
        self.wallets.rekey(wallet_id, password)
    }

    pub fn lock_wallet(&self, wallet_id: &WalletId) -> Result<(), WalletsError> {
        self.wallets.lock(wallet_id)
    }

    pub fn enter_wallet_password(
        &self,
        wallet_id: WalletId,
        password: &str,
    ) -> Result<(), WalletsError> {
        self.wallets.enter_password(wallet_id, password)
    }

    pub fn import_wallet(&self, wallet_id: WalletId, contents: &str) -> anyhow::Result<()> {
        self.wallets.import(wallet_id, contents)
    }

    pub fn import_replace(
        &self,
        wallet_id: WalletId,
        contents: &str,
        password: &str,
    ) -> anyhow::Result<()> {
        self.wallets.import_replace(wallet_id, contents, password)
    }

    pub fn accounts_of_wallet(
        &self,
        wallet_id: &WalletId,
    ) -> Result<Vec<Account>, WalletsError> {
        self.wallets.get_accounts_of_wallet(wallet_id)
    }

    pub fn clear_send_ids(&self) {
        self.wallets.clear_send_ids();
    }

    pub fn deterministic_insert(
        &self,
        wallet_id: &WalletId,
        generate_work: bool,
    ) -> Result<PublicKey, WalletsError> {
        self.wallets.deterministic_insert2(wallet_id, generate_work)
    }

    pub fn deterministic_insert_at(
        &self,
        wallet_id: &WalletId,
        index: u32,
        generate_work: bool,
    ) -> Result<PublicKey, WalletsError> {
        self.wallets
            .deterministic_insert_at(wallet_id, index, generate_work)
    }

    pub fn deterministic_index_get(&self, wallet_id: &WalletId) -> Result<u32, WalletsError> {
        self.wallets.deterministic_index_get(wallet_id)
    }

    pub fn fetch(
        &self,
        wallet_id: &WalletId,
        account: &PublicKey,
    ) -> Result<RawKey, WalletsError> {
        self.wallets.fetch(wallet_id, account)
    }

    pub fn move_accounts(
        &self,
        source_id: &WalletId,
        target_id: &WalletId,
        accounts: &[PublicKey],
    ) -> Result<(), WalletsError> {
        self.wallets.move_accounts(source_id, target_id, accounts)
    }

    pub fn insert_adhoc(
        &self,
        wallet_id: &WalletId,
        key: &RawKey,
        generate_work: bool,
    ) -> Result<PublicKey, WalletsError> {
        self.wallets.insert_adhoc2(wallet_id, key, generate_work)
    }

    pub fn insert_watch(
        &self,
        wallet_id: &WalletId,
        accounts: &[Account],
    ) -> Result<(), WalletsError> {
        self.wallets.insert_watch(wallet_id, accounts)
    }

    pub fn remove_key(
        &self,
        wallet_id: &WalletId,
        account: &PublicKey,
    ) -> Result<(), WalletsError> {
        self.wallets.remove_key(wallet_id, account)
    }

    pub fn wallet_seed(&self, wallet_id: WalletId) -> Result<RawKey, WalletsError> {
        self.wallets.get_seed(wallet_id)
    }

    pub fn change_wallet_seed(
        &self,
        wallet_id: WalletId,
        seed: &RawKey,
        count: u32,
    ) -> Result<(u32, Account), WalletsError> {
        self.wallets.change_seed(wallet_id, seed, count)
    }

    pub fn wallet_representative(&self, wallet_id: WalletId) -> Result<PublicKey, WalletsError> {
        self.wallets.get_representative(wallet_id)
    }

    pub fn set_wallet_representative(
        &self,
        wallet_id: WalletId,
        representative: PublicKey,
        update_existing_accounts: bool,
    ) -> MultiBlockPromise {
        self.wallets
            .set_representative(wallet_id, representative, update_existing_accounts)
    }

    pub fn valid_password(&self, wallet_id: &WalletId) -> Result<bool, WalletsError> {
        self.wallets.valid_password(wallet_id)
    }

    pub fn work_get(
        &self,
        wallet_id: &WalletId,
        account: &PublicKey,
    ) -> Result<WorkNonce, WalletsError> {
        self.wallets.work_get2(wallet_id, account)
    }

    pub fn work_set(
        &self,
        wallet_id: &WalletId,
        account: &PublicKey,
        work: WorkNonce,
    ) -> Result<(), WalletsError> {
        self.wallets.work_set(wallet_id, account, work)
    }

    pub fn decrypt_wallet(
        &self,
        wallet_id: WalletId,
    ) -> Result<Vec<(PublicKey, RawKey)>, WalletsError> {
        self.wallets.decrypt(wallet_id)
    }

    pub fn serialize_wallet(&self, wallet_id: WalletId) -> Result<String, WalletsError> {
        self.wallets.serialize(wallet_id)
    }

    pub fn search_receivable_all(&self) {
        self.wallets.search_receivable_all();
    }

    pub fn search_receivable(&self, wallet_id: &WalletId) -> MultiBlockPromise {
        self.wallets.search_receivable(wallet_id)
    }

    pub fn key_type(&self, wallet_id: WalletId, account: &PublicKey) -> KeyType {
        self.wallets.key_type(wallet_id, account)
    }

    pub fn send(
        &self,
        wallet_id: WalletId,
        source: Account,
        destination: Account,
        amount: Amount,
        work: WorkNonce,
        generate_work: bool,
        id: Option<String>,
    ) -> BlockPromise {
        self.wallets
            .send(wallet_id, source, destination, amount, work, generate_work, id)
    }

    pub fn receive(
        &self,
        wallet_id: WalletId,
        send_hash: BlockHash,
        representative: PublicKey,
        amount: Amount,
        account: Account,
        work: WorkNonce,
        generate_work: bool,
    ) -> BlockPromise {
        self.wallets
            .receive(
                wallet_id,
                send_hash,
                representative,
                amount,
                account,
                work,
                generate_work,
            )
    }

    pub fn account_exists(&self, account: &PublicKey) -> bool {
        self.wallets.exists(account)
    }

    #[cfg(any(test, feature = "test_support"))]
    #[doc(hidden)]
    pub fn wallets_arc(&self) -> Arc<Wallets> {
        self.wallets.clone()
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
pub(crate) struct NodeServiceBundle {
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
    #[cfg(feature = "ledger_snapshots")]
    pub ledger_snapshots: Arc<LedgerSnapshots>,
}

#[cfg(any(test, feature = "test_support"))]
pub(crate) type NodeServices = NodeServiceBundle;

impl NodeServiceBundle {
    pub(crate) fn ledger(&self) -> Arc<Ledger> {
        self.ledger.clone()
    }

    pub(crate) fn stats(&self) -> Arc<Stats> {
        self.stats.clone()
    }

    pub(crate) fn work_factory(&self) -> Arc<WorkFactory> {
        self.work_factory.clone()
    }

    pub(crate) fn wallet_reps(&self) -> Arc<Mutex<WalletRepresentatives>> {
        self.wallet_reps.clone()
    }

    pub(crate) fn network_components(
        &self,
    ) -> (
        Arc<RwLock<Network>>,
        Arc<TcpListener>,
        Arc<PeerConnector>,
        Arc<Mutex<NetworkThreads>>,
        Arc<Mutex<MessageProcessor>>,
        Arc<Mutex<MessageSender>>,
        Arc<Mutex<MessageFlooder>>,
        Arc<KeepalivePublisher>,
        Arc<InboundMessageQueue>,
        Arc<NetworkFilter>,
        Arc<SteadyClock>,
    ) {
        (
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

    #[allow(clippy::type_complexity)]
    pub(crate) fn consensus_components(
        &self,
    ) -> (
        Arc<RwLock<ActiveElectionsContainer>>,
        Arc<ElectionSchedulers>,
        Arc<VoteProcessor>,
        Arc<VoteGenerators>,
        Arc<LocalVoteHistory>,
        Arc<RequestAggregator>,
        Arc<BoundedBacklog>,
        Arc<Bootstrapper>,
        Arc<RepCrawler>,
        Arc<Mutex<OnlineReps>>,
        Arc<CurrentRepTiers>,
        Arc<LocalBlockBroadcaster>,
        Arc<Mutex<WinnerBlockBroadcaster>>,
        Arc<VoteProcessorQueue>,
        Arc<Mutex<VoteCache>>,
        Arc<VoteCacheProcessor>,
        Arc<ConfirmingSet>,
        Arc<BlockProcessor>,
        Arc<BlockProcessorQueue>,
        Arc<Mutex<VoteRebroadcaster>>,
    ) {
        (
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

    pub(crate) fn telemetry_components(&self) -> (Arc<Telemetry>, Arc<TcpListener>) {
        (self.telemetry.clone(), self.tcp_listener.clone())
    }

    pub(crate) fn bootstrap_components(
        &self,
    ) -> (Arc<Bootstrapper>, Arc<BootstrapServer>, Arc<WorkFactory>) {
        (
            self.bootstrapper.clone(),
            self.bootstrap_server.clone(),
            self.work_factory.clone(),
        )
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
pub struct LedgerQueryServices {
    ledger: Arc<Ledger>,
    ledger_queries: LedgerQueryHandle,
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
    #[cfg(any(test, feature = "test_support"))]
    pub fn ledger_arc(&self) -> Arc<Ledger> {
        self.ledger.clone()
    }

    pub fn ledger_queries(&self) -> LedgerQueryHandle {
        self.ledger_queries.clone()
    }

    pub(crate) fn new(
        ledger: Arc<Ledger>,
        block_rates: Arc<CurrentBlockRates>,
        confirming_set: Arc<ConfirmingSet>,
        recently_cemented: Arc<Mutex<BoundedVecDeque<ConfirmedElection>>>,
        stats: Arc<Stats>,
    ) -> Self {
        let ledger_queries = LedgerQueryHandle::new(ledger.clone());
        Self {
            ledger,
            ledger_queries,
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
