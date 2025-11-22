use std::{
    ops::{Deref, DerefMut},
    sync::{Arc, Mutex},
    time::Duration,
};

use bounded_vec_deque::BoundedVecDeque;

use crate::{
    block_processing::BacklogScan,
    block_rate_calculator::CurrentBlockRates,
    bootstrap::{BootstrapExt, BootstrapServer, Bootstrapper},
    cementation::ConfirmingSet,
    config::{NetworkParams, NodeFlags},
    consensus::{
        AecTicker, AecVoter, election::ConfirmedElection,
    },
    telemetry::{TelementryExt, Telemetry},
    handles::LedgerQueryHandle,
    wallets::WalletRepresentatives,
    work::WorkFactory,
};
use rsnano_ledger::Ledger;
use rsnano_network::TcpListener;
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

    pub fn trigger(&self) {
        self.backlog_scan.trigger();
    }

    pub fn notify(&self) {
        self.backlog_scan.trigger();
    }

    pub fn start(&mut self) {
        self.backlog_scan.start();
    }

    pub fn stop(&mut self) {
        self.backlog_scan.stop();
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
