use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddrV6,
    ops::{Deref, DerefMut},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::SystemTime,
};

use tracing::debug;

use rsnano_types::{
    Account, AccountInfo, Amount, Block, BlockHash, ConfirmationHeightInfo, Epoch, Link,
    PendingInfo, PendingKey, PublicKey, QualifiedRoot, Root, SavedBlock, UnixTimestamp,
};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{DetailType, StatType, Stats},
};
use rsnano_work_validation::WorkThresholds;

use crate::{
    BlockRollbackPerformer, BorrowingAnySet, BorrowingConfirmedSet, GenerateCacheFlags,
    LedgerConstants, LedgerSet, LedgerStore, OwningAnySet, OwningConfirmedSet,
    OwningUnconfirmedSet, RepWeightCache, RepWeightWriterStats, RepWeightsUpdater, RollbackError,
    block_cementer::BlockCementer,
    block_insertion::{BlockInsertInstructions, BlockInserter, BlockValidatorFactory},
    deferred_operations::DeferredLedgerOperations,
    iterator_metrics::{IteratorMetricsConfig, LedgerIteratorMetrics},
    vote_verifier::VoteVerifier,
};
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use store_traits::{
    LedgerReadTxn, LedgerWriteTxn,
    ledger::{LedgerStoreFactory, MemoryStats, WriteStrategy, WriterType},
    types::{StoreError, StoreErrorKind},
};

#[derive(PartialEq, Eq, Debug, Clone, Copy, EnumCount, EnumIter, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum BlockError {
    /// Signature was bad, forged or transmission error
    BadSignature,
    /// Already seen and was valid
    Old,
    /// Malicious attempt to spend a negative amount
    NegativeSpend,
    /// Malicious fork based on previous
    Fork,
    /// Source block doesn't exist, has already been received, or requires an account upgrade (epoch blocks)
    Unreceivable,
    /// Block marked as previous is unknown
    GapPrevious,
    /// Block marked as source is unknown
    GapSource,
    /// Block marked as pending blocks required for epoch open block are unknown
    GapEpochOpenPending,
    /// Block attempts to open the burn account
    OpenedBurnAccount,
    /// Balance and amount delta don't match
    BalanceMismatch,
    /// Representative is changed when it is not allowed
    RepresentativeMismatch,
    /// This block cannot follow the previous block
    BlockPosition,
    /// Insufficient work for this block, even though it passed the minimal validation
    InsufficientWork,
    /// The account got updated while this block was processed. This block is ether old or a fork.
    Conflict,
}

impl BlockError {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlockError::BadSignature => "Bad signature",
            BlockError::Old => "Old",
            BlockError::NegativeSpend => "Negative spend",
            BlockError::Fork => "Fork",
            BlockError::Unreceivable => "Unreceivable",
            BlockError::GapPrevious => "Gap previous",
            BlockError::GapSource => "Gap source",
            BlockError::GapEpochOpenPending => "Gap epoch open pendign",
            BlockError::OpenedBurnAccount => "Opened burn account",
            BlockError::BalanceMismatch => "Balance mismatch",
            BlockError::RepresentativeMismatch => "Representative mismatch",
            BlockError::BlockPosition => "Block position",
            BlockError::InsufficientWork => "Insufficient work",
            BlockError::Conflict => "Conflict",
        }
    }
}

impl From<BlockError> for DetailType {
    fn from(value: BlockError) -> Self {
        match value {
            BlockError::BadSignature => Self::BadSignature,
            BlockError::Old => Self::Old,
            BlockError::NegativeSpend => Self::NegativeSpend,
            BlockError::Fork => Self::Fork,
            BlockError::Unreceivable => Self::Unreceivable,
            BlockError::GapPrevious => Self::GapPrevious,
            BlockError::GapSource => Self::GapSource,
            BlockError::GapEpochOpenPending => Self::GapEpochOpenPending,
            BlockError::OpenedBurnAccount => Self::OpenedBurnAccount,
            BlockError::BalanceMismatch => Self::BalanceMismatch,
            BlockError::RepresentativeMismatch => Self::RepresentativeMismatch,
            BlockError::BlockPosition => Self::BlockPosition,
            BlockError::InsufficientWork => Self::InsufficientWork,
            BlockError::Conflict => Self::Conflict,
        }
    }
}

pub struct Ledger {
    pub store: Arc<dyn LedgerStore>,
    pub rep_weights_updater: RepWeightsUpdater,
    pub rep_weights: Arc<RepWeightCache>,
    pub constants: LedgerConstants,
    pub(crate) stats: Arc<Stats>,
    ledger_metrics: Option<Arc<LedgerIteratorMetrics>>,
    block_count_events: Arc<BlockCountEvents>,
    optimistic_successes: AtomicU64,
    optimistic_conflicts: AtomicU64,
    pessimistic_fallbacks: AtomicU64,
    insert_tracker: Mutex<InsertTracker>,
    duplicate_log: Mutex<VecDeque<DuplicateInsertRecord>>,
    rollback_listener: OutputListenerMt<BlockHash>,
}

pub(crate) struct BlockCountEvents {
    inserts: AtomicU64,
    rollbacks: AtomicU64,
    duplicate_inserts: AtomicU64,
    insert_sources: Vec<AtomicU64>,
    duplicate_sources: Vec<AtomicU64>,
}

const MAX_INSERT_SOURCES: usize = 32;
const MAX_TRACKED_INSERTS: usize = 600_000;
const MAX_DUPLICATE_LOG: usize = 1024;
pub const DEFAULT_OPTIMISTIC_RETRIES: usize = 1;

struct InsertTracker {
    map: HashMap<BlockHash, u8>,
    order: VecDeque<BlockHash>,
}

impl InsertTracker {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn record_insert(&mut self, hash: BlockHash, source: u8) -> Option<DuplicateInsertRecord> {
        if let Some(prev) = self.map.get(&hash).copied() {
            Some(DuplicateInsertRecord::new(hash, prev, source))
        } else {
            self.map.insert(hash, source);
            self.order.push_back(hash);
            if self.order.len() > MAX_TRACKED_INSERTS {
                if let Some(old) = self.order.pop_front() {
                    self.map.remove(&old);
                }
            }
            None
        }
    }

    fn check_existing(&self, hash: &BlockHash, source: u8) -> Option<DuplicateInsertRecord> {
        self.map
            .get(hash)
            .copied()
            .map(|prev| DuplicateInsertRecord::new(*hash, prev, source))
    }
}

struct DuplicateInsertRecord {
    hash: BlockHash,
    first_source: u8,
    second_source: u8,
    timestamp: SystemTime,
}

impl DuplicateInsertRecord {
    fn new(hash: BlockHash, first_source: u8, second_source: u8) -> Self {
        Self {
            hash,
            first_source,
            second_source,
            timestamp: SystemTime::now(),
        }
    }
}

#[derive(Clone)]
pub struct DuplicateInsertRecordSnapshot {
    pub hash: BlockHash,
    pub first_source: u8,
    pub second_source: u8,
    pub timestamp: SystemTime,
}

pub enum CommitDisposition {
    Success,
    Duplicate,
}

impl Default for BlockCountEvents {
    fn default() -> Self {
        let mut insert_sources = Vec::with_capacity(MAX_INSERT_SOURCES);
        let mut duplicate_sources = Vec::with_capacity(MAX_INSERT_SOURCES);
        for _ in 0..MAX_INSERT_SOURCES {
            insert_sources.push(AtomicU64::new(0));
            duplicate_sources.push(AtomicU64::new(0));
        }
        Self {
            inserts: AtomicU64::new(0),
            rollbacks: AtomicU64::new(0),
            duplicate_inserts: AtomicU64::new(0),
            insert_sources,
            duplicate_sources,
        }
    }
}

impl BlockCountEvents {
    pub(crate) fn record_insert(&self) {
        self.inserts.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn record_insert_source(&self, source: u8) {
        let index = (source as usize).min(self.insert_sources.len() - 1);
        self.insert_sources[index].fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn record_rollback(&self) {
        self.rollbacks.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn record_duplicate_insert(&self) {
        self.duplicate_inserts.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn record_duplicate_source(&self, source: u8) {
        let index = (source as usize).min(self.duplicate_sources.len() - 1);
        self.duplicate_sources[index].fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn inserts(&self) -> u64 {
        self.inserts.load(Ordering::SeqCst)
    }

    pub(crate) fn rollbacks(&self) -> u64 {
        self.rollbacks.load(Ordering::SeqCst)
    }

    pub(crate) fn insert_sources(&self) -> Vec<u64> {
        self.insert_sources
            .iter()
            .map(|counter| counter.load(Ordering::SeqCst))
            .collect()
    }

    pub(crate) fn duplicate_inserts(&self) -> u64 {
        self.duplicate_inserts.load(Ordering::SeqCst)
    }

    pub(crate) fn duplicate_sources(&self) -> Vec<u64> {
        self.duplicate_sources
            .iter()
            .map(|counter| counter.load(Ordering::SeqCst))
            .collect()
    }
}

pub struct NullLedgerBuilder {
    store_factory: Arc<dyn LedgerStoreFactory>,
    blocks: Vec<SavedBlock>,
    accounts: Vec<(Account, AccountInfo)>,
    pending: Vec<(PendingKey, PendingInfo)>,
    peers: Vec<(SocketAddrV6, SystemTime)>,
    #[cfg(feature = "ledger_snapshots")]
    forks: Vec<(QualifiedRoot, SnapshotNumber)>,
    confirmation_height: Vec<(Account, ConfirmationHeightInfo)>,
    min_rep_weight: Amount,
}

impl NullLedgerBuilder {
    fn new(store_factory: Arc<dyn LedgerStoreFactory>) -> Self {
        Self {
            store_factory,
            blocks: Vec::new(),
            accounts: Vec::new(),
            pending: Vec::new(),
            peers: Vec::new(),
            #[cfg(feature = "ledger_snapshots")]
            forks: Vec::new(),
            confirmation_height: Vec::new(),
            min_rep_weight: Amount::ZERO,
        }
    }

    pub fn store_factory(mut self, factory: Arc<dyn LedgerStoreFactory>) -> Self {
        self.store_factory = factory;
        self
    }

    pub fn block(mut self, block: &SavedBlock) -> Self {
        self.blocks.push(block.clone());
        self
    }

    pub fn blocks<'a>(mut self, blocks: impl IntoIterator<Item = &'a SavedBlock>) -> Self {
        for b in blocks.into_iter() {
            self.blocks.push(b.clone());
        }
        self
    }

    pub fn peers(mut self, peers: impl IntoIterator<Item = (SocketAddrV6, SystemTime)>) -> Self {
        for (peer, time) in peers.into_iter() {
            self.peers.push((peer, time))
        }
        self
    }

    pub fn confirmation_height(mut self, account: &Account, info: &ConfirmationHeightInfo) -> Self {
        self.confirmation_height.push((*account, info.clone()));
        self
    }

    pub fn account_info(mut self, account: &Account, info: &AccountInfo) -> Self {
        self.accounts.push((*account, info.clone()));
        self
    }

    pub fn pending(mut self, key: &PendingKey, info: &PendingInfo) -> Self {
        self.pending.push((key.clone(), info.clone()));
        self
    }

    #[cfg(feature = "ledger_snapshots")]
    pub fn fork(mut self, root: &QualifiedRoot, snapshot_number: SnapshotNumber) -> Self {
        self.forks.push((root.clone(), snapshot_number));
        self
    }

    pub fn frontiers(self, frontiers: impl IntoIterator<Item = (Account, BlockHash)>) -> Self {
        let mut builder = self;

        for (account, frontier) in frontiers {
            builder = builder
                .account_info(&account, &AccountInfo::new_test_instance())
                .confirmation_height(
                    &account,
                    &ConfirmationHeightInfo {
                        height: 0,
                        frontier,
                    },
                );
        }

        builder
    }

    #[cfg(feature = "ledger_snapshots")]
    pub fn forks(self, forks: impl IntoIterator<Item = (QualifiedRoot, SnapshotNumber)>) -> Self {
        let mut builder = self;

        for (root, snapshot_number) in forks {
            builder = builder.fork(&root, snapshot_number);
        }

        builder
    }

    pub fn finish(self) -> Ledger {
        let Self {
            store_factory,
            blocks,
            accounts,
            pending,
            peers,
            #[cfg(feature = "ledger_snapshots")]
            forks,
            confirmation_height,
            min_rep_weight,
        } = self;

        let rep_weights = Arc::new(RepWeightCache::new());
        let store = store_factory
            .create_null_store(rep_weights.ledger_cache.clone())
            .unwrap();
        Self::seed_store(
            store.as_ref(),
            &blocks,
            &accounts,
            &pending,
            &peers,
            &confirmation_height,
            #[cfg(feature = "ledger_snapshots")]
            &forks,
        );

        Ledger::new(
            store,
            LedgerConstants::unit_test(),
            min_rep_weight,
            rep_weights,
            Arc::new(Stats::default()),
            1,
        )
        .unwrap()
    }

    fn seed_store(
        store: &dyn LedgerStore,
        blocks: &[SavedBlock],
        accounts: &[(Account, AccountInfo)],
        pending: &[(PendingKey, PendingInfo)],
        peers: &[(SocketAddrV6, SystemTime)],
        confirmation_height: &[(Account, ConfirmationHeightInfo)],
        #[cfg(feature = "ledger_snapshots")] forks: &[(QualifiedRoot, SnapshotNumber)],
    ) {
        #[cfg(feature = "ledger_snapshots")]
        let has_forks = !forks.is_empty();
        #[cfg(not(feature = "ledger_snapshots"))]
        let has_forks: bool = false;

        if blocks.is_empty()
            && accounts.is_empty()
            && pending.is_empty()
            && peers.is_empty()
            && confirmation_height.is_empty()
            && !has_forks
        {
            return;
        }

        let mut txn = store.begin_write();

        for block in blocks {
            store.block().put(txn.as_mut(), block);
            if !block.previous().is_zero() {
                store
                    .successors()
                    .put(txn.as_mut(), &block.previous(), &block.hash());
            }
        }

        for (account, info) in accounts {
            store.account().put(txn.as_mut(), account, info);
        }

        for (account, info) in confirmation_height {
            store.confirmation_height().put(txn.as_mut(), account, info);
        }

        for (key, info) in pending {
            store.pending().put(txn.as_mut(), key, info);
        }

        for (peer, time) in peers {
            store.peer().put(txn.as_mut(), peer.clone(), time.clone());
        }

        #[cfg(feature = "ledger_snapshots")]
        for (root, snapshot_number) in forks {
            store.forks().put(txn.as_mut(), root, *snapshot_number);
        }

        txn.commit()
            .unwrap_or_else(|e| panic!("failed to commit ledger bootstrap txn: {e}"));
    }
}

impl Ledger {
    pub fn new_null(store_factory: Arc<dyn LedgerStoreFactory>) -> Self {
        let rep_weights = Arc::new(RepWeightCache::new());
        let store = store_factory
            .create_null_store(rep_weights.ledger_cache.clone())
            .unwrap();

        Self::new(
            store,
            LedgerConstants::unit_test(),
            Amount::ZERO,
            rep_weights,
            Arc::new(Stats::default()),
            1,
        )
        .unwrap()
    }

    pub fn new_null_builder(store_factory: Arc<dyn LedgerStoreFactory>) -> NullLedgerBuilder {
        NullLedgerBuilder::new(store_factory)
    }

    pub(crate) fn store_ref(&self) -> &dyn LedgerStore {
        self.store.as_ref()
    }

    pub fn optimistic_successes(&self) -> u64 {
        self.optimistic_successes.load(Ordering::SeqCst)
    }

    pub fn optimistic_conflicts(&self) -> u64 {
        self.optimistic_conflicts.load(Ordering::SeqCst)
    }

    pub fn pessimistic_fallbacks(&self) -> u64 {
        self.pessimistic_fallbacks.load(Ordering::SeqCst)
    }

    pub fn rep_weight_writer_stats(&self) -> Arc<RepWeightWriterStats> {
        self.rep_weights_updater.stats().clone()
    }

    pub fn apply_rep_weight_ops<F>(&self, mut f: F)
    where
        F: FnMut(&mut dyn LedgerWriteTxn),
    {
        let stats = self.rep_weights_updater.stats();
        let _guard = stats.start_optimistic_writer();
        let prev_successes = self.optimistic_successes();
        let prev_conflicts = self.optimistic_conflicts();
        let prev_fallbacks = self.pessimistic_fallbacks();

        self.tx_optimistic_process(
            WriterType::RepWeightUpdater,
            DEFAULT_OPTIMISTIC_RETRIES,
            |txn, _deferred| {
                f(txn);
                Ok(((), Vec::new()))
            },
        )
        .unwrap_or_else(|e| panic!("failed to apply rep weight ops: {e}"));

        stats.add_deltas(
            self.optimistic_successes()
                .saturating_sub(prev_successes),
            self.optimistic_conflicts()
                .saturating_sub(prev_conflicts),
            self.pessimistic_fallbacks()
                .saturating_sub(prev_fallbacks),
        );
    }

    pub fn begin_write_with(
        &self,
        writer: WriterType,
        strategy: WriteStrategy,
    ) -> Box<dyn LedgerWriteTxn> {
        self.store.begin_write_with_writer(writer, strategy)
    }

    pub(crate) fn block_count_events_arc(&self) -> Arc<BlockCountEvents> {
        Arc::clone(&self.block_count_events)
    }

    pub fn validate_batch<'a>(
        &self,
        batch: impl IntoIterator<Item = &'a Block>,
    ) -> Vec<(Result<BlockInsertInstructions, BlockError>, Block)> {
        let mut validation_results = Vec::new();
        let tx = self.store.begin_read();
        let metrics = self.iterator_metrics();
        for block in batch.into_iter() {
            let any = BorrowingAnySet {
                constants: &self.constants,
                store: self.store_ref(),
                tx: tx.as_ref(),
                metrics: metrics.clone(),
            };
            let validator = BlockValidatorFactory::new(&any, &self.constants, block).create_validator();
            let result = validator.validate();
            validation_results.push((result, block.clone()));
        }
        validation_results
    }

    pub fn apply_validated_batch(
        &self,
        txn: &mut dyn LedgerWriteTxn,
        deferred: &mut DeferredLedgerOperations,
        validation_results: &[(Result<BlockInsertInstructions, BlockError>, Block)],
    ) -> (Vec<BatchProcessEntry>, Vec<BlockHash>) {
        let mut processed = Vec::with_capacity(validation_results.len());
        let mut inserted_hashes = Vec::new();
        for (result, block) in validation_results.iter() {
            match result {
                Ok(instructions) => {
                    let mut block_clone = block.clone();
                    let instructions_clone = instructions.clone();
                    let (saved_block, inserted, preexisting) =
                        BlockInserter::new(self, txn, &mut block_clone, &instructions_clone)
                            .insert(deferred);
                    if let Some(saved_block) = saved_block {
                        if inserted {
                            inserted_hashes.push(saved_block.hash());
                        }
                        processed.push(BatchProcessEntry {
                            status: Ok(()),
                            saved_block: Some(saved_block),
                            inserted,
                            preexisting,
                        });
                    } else {
                        processed.push(BatchProcessEntry {
                            status: Err(BlockError::Conflict),
                            saved_block: None,
                            inserted: false,
                            preexisting: false,
                        });
                    }
                }
                Err(err) => {
                    processed.push(BatchProcessEntry {
                        status: Err(*err),
                        saved_block: None,
                        inserted: false,
                        preexisting: false,
                    });
                }
            }
        }
        (processed, inserted_hashes)
    }

    pub fn tx_optimistic_process<T, F>(
        &self,
        writer: WriterType,
        max_retries: usize,
        mut f: F,
    ) -> Result<T, StoreError>
    where
        F: FnMut(
            &mut dyn LedgerWriteTxn,
            &mut DeferredLedgerOperations,
        ) -> Result<(T, Vec<BlockHash>), StoreError>,
    {
        let mut retries = 0;
        let mut strategy = WriteStrategy::Optimistic;
        loop {
            let mut deferred = DeferredLedgerOperations::new();
            let mut txn = self.begin_write_with(writer, strategy);
            let (result, inserted_hashes) = f(txn.as_mut(), &mut deferred)?;
            match self.commit_block_transaction(txn, &inserted_hashes) {
                Ok(CommitDisposition::Success) => {
                    if matches!(strategy, WriteStrategy::Optimistic) {
                        self.optimistic_successes.fetch_add(1, Ordering::SeqCst);
                    } else {
                        self.pessimistic_fallbacks.fetch_add(1, Ordering::SeqCst);
                    }
                    deferred.execute(self);
                    return Ok(result);
                }
                Ok(CommitDisposition::Duplicate) => {
                    if matches!(strategy, WriteStrategy::Optimistic) {
                        self.optimistic_conflicts.fetch_add(1, Ordering::SeqCst);
                        if retries < max_retries {
                            retries += 1;
                            continue;
                        } else {
                            strategy = WriteStrategy::Pessimistic;
                            continue;
                        }
                    } else {
                        self.pessimistic_fallbacks.fetch_add(1, Ordering::SeqCst);
                        return Ok(result);
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub fn commit_block_transaction(
        &self,
        txn: Box<dyn LedgerWriteTxn>,
        inserted_hashes: &[BlockHash],
    ) -> Result<CommitDisposition, StoreError> {
        match txn.commit() {
            Ok(()) => Ok(CommitDisposition::Success),
            Err(e) if e.kind() == StoreErrorKind::Conflict => {
                for _hash in inserted_hashes {
                    self.record_duplicate_insert_event();
                }
                Ok(CommitDisposition::Duplicate)
            }
            Err(e) => Err(e),
        }
    }

    pub(crate) fn new(
        store: Arc<dyn LedgerStore>,
        constants: LedgerConstants,
        min_rep_weight: Amount,
        rep_weights: Arc<RepWeightCache>,
        stats: Arc<Stats>,
        thread_count: usize,
    ) -> anyhow::Result<Self> {
        let rep_weights_updater =
            RepWeightsUpdater::new(store.rep_weight_store(), min_rep_weight, &rep_weights);

        let mut ledger = Self {
            rep_weights,
            rep_weights_updater,
            store,
            constants,
            stats,
            ledger_metrics: None,
            block_count_events: Arc::new(BlockCountEvents::default()),
            optimistic_successes: AtomicU64::new(0),
            optimistic_conflicts: AtomicU64::new(0),
            pessimistic_fallbacks: AtomicU64::new(0),
            insert_tracker: Mutex::new(InsertTracker::new()),
            duplicate_log: Mutex::new(VecDeque::with_capacity(MAX_DUPLICATE_LOG)),
            rollback_listener: Default::default(),
        };

        ledger.initialize(thread_count, &GenerateCacheFlags::new())?;

        Ok(ledger)
    }

    pub fn update_metrics_config(&mut self, config: IteratorMetricsConfig) {
        if config.enabled {
            let metrics = Arc::new(LedgerIteratorMetrics::new(Arc::clone(&self.stats), config));
            self.ledger_metrics = Some(metrics);
        } else {
            self.ledger_metrics = None;
        }
    }

    pub(crate) fn iterator_metrics(&self) -> Option<Arc<LedgerIteratorMetrics>> {
        self.ledger_metrics.clone()
    }

    fn initialize(
        &mut self,
        thread_count: usize,
        generate_cache: &GenerateCacheFlags,
    ) -> anyhow::Result<()> {
        if {
            let tx = self.store.begin_read();
            self.store.account().iter(tx.as_ref()).next().is_none()
        } {
            let mut txn = self.store_ref().begin_write();
            self.add_genesis_block(txn.as_mut());
            txn.commit()
                .unwrap_or_else(|e| panic!("failed to commit genesis block: {e}"));
        }

        if generate_cache.reps || generate_cache.account_count || generate_cache.block_count {
            self.store.for_each_account_par(thread_count, &|iter| {
                let mut block_count = 0;
                let mut account_count = 0;
                let mut rep_weights: HashMap<PublicKey, Amount> = HashMap::new();

                for (_, info) in iter {
                    block_count += info.block_count;
                    account_count += 1;
                    if !info.balance.is_zero() {
                        let total = rep_weights.entry(info.representative).or_default();
                        *total += info.balance;
                    }
                }
                self.store
                    .cache()
                    .block_count
                    .fetch_add(block_count, Ordering::SeqCst);

                self.store
                    .cache()
                    .account_count
                    .fetch_add(account_count, Ordering::SeqCst);

                self.rep_weights_updater.copy_from(&rep_weights);
            });
        }

        if generate_cache.confirmed_count {
            self.store
                .for_each_confirmation_height_par(thread_count, &|iter| {
                    let mut confirmed_count = 0;
                    for (_, info) in iter {
                        confirmed_count += info.height;
                    }
                    self.store
                        .cache()
                        .confirmed_count
                        .fetch_add(confirmed_count, Ordering::SeqCst);
                });
        }

        Ok(())
    }

    fn add_genesis_block(&self, txn: &mut dyn LedgerWriteTxn) {
        let genesis_hash = self.constants.genesis_block.hash();
        let genesis_account = self.constants.genesis_account;
        self.store.block().put(txn, &self.constants.genesis_block);

        self.store.confirmation_height().put(
            txn,
            &genesis_account,
            &ConfirmationHeightInfo::new(1, genesis_hash),
        );

        self.store.account().put(
            txn,
            &genesis_account,
            &AccountInfo {
                head: genesis_hash,
                representative: genesis_account.into(),
                open_block: genesis_hash,
                balance: u128::MAX.into(),
                modified: UnixTimestamp::ZERO,
                block_count: 1,
                epoch: Epoch::Epoch0,
            },
        );
        self.store
            .rep_weight()
            .put(txn, genesis_account.into(), Amount::MAX);
    }

    pub fn any(&self) -> OwningAnySet<'_> {
        OwningAnySet::new_with_metrics(self.store_ref(), &self.constants, self.iterator_metrics())
    }

    pub fn confirmed(&self) -> OwningConfirmedSet<'_> {
        let tx = self.store_ref().begin_read();
        OwningConfirmedSet::new(self.store_ref(), tx)
    }

    pub fn unconfirmed(&self) -> impl LedgerSet + use<'_> {
        let tx = self.store_ref().begin_read();
        OwningUnconfirmedSet::new(self.store_ref(), tx)
    }

    pub fn bootstrap_weight_max_blocks(&self) -> u64 {
        self.rep_weights.bootstrap_weight_max_blocks()
    }

    /// Returns the cached vote weight for the given representative.
    /// If the weight is below the cache limit it returns 0.
    /// During bootstrap it returns the preconfigured bootstrap weights.
    pub fn weight(&self, rep: &PublicKey) -> Amount {
        self.rep_weights.weight(rep)
    }

    pub fn is_epoch_link(&self, link: &Link) -> bool {
        self.constants.epochs.is_epoch_link(link)
    }

    pub fn epoch_signer(&self, link: &Link) -> Option<Account> {
        self.constants.epochs.epoch_signer(link)
    }

    pub fn epoch_link(&self, epoch: Epoch) -> Option<Link> {
        self.constants.epochs.link(epoch).cloned()
    }

    pub(crate) fn update_account(
        &self,
        txn: &mut dyn LedgerWriteTxn,
        account: &Account,
        old_info: &AccountInfo,
        new_info: &AccountInfo,
    ) {
        if !new_info.head.is_zero() {
            if old_info.head.is_zero() && new_info.open_block == new_info.head {
                self.store
                    .cache()
                    .account_count
                    .fetch_add(1, Ordering::SeqCst);
            }
            if !old_info.head.is_zero() && old_info.epoch != new_info.epoch {
                // store.account() ().put won't erase existing entries if they're in different tables
                self.store.account().del(txn, account);
            }
            self.store.account().put(txn, account, new_info);
        } else {
            debug_assert!(!self.store.confirmation_height().exists(txn, account));
            self.store.account().del(txn, account);
            debug_assert!(self.store.cache().account_count.load(Ordering::SeqCst) > 0);
            self.store
                .cache()
                .account_count
                .fetch_sub(1, Ordering::SeqCst);
        }
    }

    pub fn track_rollbacks(&self) -> Arc<OutputTrackerMt<BlockHash>> {
        self.rollback_listener.track()
    }

    /// Rollback blocks until `block' doesn't exist or it tries to penetrate the confirmation height
    pub fn roll_back(&self, block: &BlockHash) -> Result<usize, RollbackError> {
        self.rollback_listener.emit(*block);
        let result = self.roll_back_batch(&[*block], usize::MAX, |_| true);
        let rolled_back = result[0].rolled_back.len();
        result[0].error.map_or(Ok(rolled_back), |e| Err(e))
    }

    pub fn roll_back_batch<'a, T, F>(
        &self,
        targets: T,
        max_rollbacks: usize,
        mut can_roll_back: F,
    ) -> RollbackResults
    where
        T: IntoIterator<Item = &'a BlockHash>,
        F: FnMut(&BlockHash) -> bool,
    {
        self.stats
            .inc(StatType::BoundedBacklog, DetailType::PerformingRollbacks);

        let mut rolled_back_count = 0;
        let mut results = RollbackResults::new();
        {
            let mut txn = self.store_ref().begin_write();

            for hash in targets {
                // Skip the rollback if the block is being used by the node, this should be race free as it's checked while holding the ledger write lock
                if !can_roll_back(hash) {
                    self.stats
                        .inc(StatType::BoundedBacklog, DetailType::RollbackSkipped);
                    results.push(RollbackResult {
                        target_hash: *hash,
                        target_root: QualifiedRoot::ZERO,
                        rolled_back: Vec::new(),
                        error: Some(RollbackError::Rejected),
                    });
                    continue;
                }

                // Here we check that the block is still OK to rollback, there could be a delay between gathering the targets and performing the rollbacks
                if let Some(block) = self.store.block().get(txn.as_ref(), hash) {
                    debug!(
                        "Rolling back: {}, account: {}",
                        hash,
                        block.account().encode_account()
                    );

                    let (rollback_list, error) =
                        self.roll_back_with_tx(txn.as_mut(), &block.hash());
                    if error.is_none() {
                        self.stats
                            .inc(StatType::BoundedBacklog, DetailType::Rollback);
                    } else {
                        self.stats
                            .inc(StatType::BoundedBacklog, DetailType::RollbackFailed);
                    }

                    rolled_back_count += rollback_list.len();
                    results.push(RollbackResult {
                        target_hash: *hash,
                        target_root: block.qualified_root(),
                        rolled_back: rollback_list,
                        error,
                    });

                    // Return early if we reached the maximum number of rollbacks
                    if rolled_back_count >= max_rollbacks {
                        break;
                    }
                } else {
                    self.stats
                        .inc(StatType::BoundedBacklog, DetailType::RollbackMissingBlock);
                    rolled_back_count += 1;
                    results.push(RollbackResult {
                        target_hash: *hash,
                        target_root: QualifiedRoot::ZERO,
                        rolled_back: Vec::new(),
                        error: Some(RollbackError::BlockNotFound),
                    });
                }
            }
            txn.commit()
                .unwrap_or_else(|e| panic!("failed to commit rollback batch: {e}"));
        }

        results
    }

    fn roll_back_with_tx(
        &self,
        tx: &mut dyn LedgerWriteTxn,
        block: &BlockHash,
    ) -> (Vec<SavedBlock>, Option<RollbackError>) {
        let mut performer = BlockRollbackPerformer::new(self, tx);
        match performer.roll_back(block) {
            Ok(()) => (performer.rolled_back, None),
            Err(e) => (performer.rolled_back, Some(e)),
        }
    }

    pub fn process_one(&self, block: &Block) -> Result<SavedBlock, BlockError> {
        let mut result = self.process_batch(std::iter::once(block));
        let mut drain = result.processed.drain(..);
        let entry = drain.next().unwrap();
        match (entry.status, entry.saved_block) {
            (Ok(_), Some(block)) => Ok(block),
            (Ok(_), None) => unreachable!(),
            (Err(e), _) => Err(e),
        }
    }

    pub fn process_batch<'a>(
        &self,
        batch: impl IntoIterator<Item = &'a Block>,
    ) -> BatchProcessResult {
        let validation_results = self.validate_batch(batch);
        let processed = self
            .tx_optimistic_process(
                WriterType::BlockProcessor,
                DEFAULT_OPTIMISTIC_RETRIES,
                |txn, deferred| {
                    let (processed, inserted_hashes) =
                        self.apply_validated_batch(txn, deferred, &validation_results);
                    Ok((processed, inserted_hashes))
                },
            )
            .unwrap_or_else(|e| panic!("failed to process block batch: {e}"));

        BatchProcessResult { processed }
    }

    pub fn roll_back_competitors<'a, T, F>(&self, blocks: T, mut rolled_back_callback: F)
    where
        T: IntoIterator<Item = &'a Block>,
        F: FnMut(RollbackResults),
    {
        let mut rolled_back = RollbackResults::new();
        {
            let mut txn = self.store_ref().begin_write();
            for block in blocks {
                if txn.is_refresh_needed() {
                    txn.commit()
                        .unwrap_or_else(|e| panic!("failed to refresh rollback txn: {e}"));
                    if !rolled_back.is_empty() {
                        rolled_back_callback(rolled_back);
                        rolled_back = RollbackResults::new();
                    }
                    txn = self.store_ref().begin_write();
                }
                let rolled_back_blocks = self.rollback_competitor(txn.as_mut(), block);
                if !rolled_back_blocks.is_empty() {
                    rolled_back.push(RollbackResult {
                        target_hash: block.hash(),
                        target_root: block.qualified_root(),
                        rolled_back: rolled_back_blocks,
                        error: None,
                    });
                }
            }
            txn.commit()
                .unwrap_or_else(|e| panic!("failed to commit competitor rollback txn: {e}"));
        }
        if !rolled_back.is_empty() {
            rolled_back_callback(rolled_back);
        }
    }

    fn rollback_competitor(
        &self,
        tx: &mut dyn LedgerWriteTxn,
        fork_block: &Block,
    ) -> Vec<SavedBlock> {
        let mut rollback_list = Vec::new();
        let hash = fork_block.hash();
        if let Some(successor) =
            self.block_successor_by_qualified_root(tx, &fork_block.qualified_root())
        {
            if successor != hash {
                // Replace our block with the winner and roll back any dependent blocks
                debug!("Rolling back: {} and replacing with: {}", successor, hash);
                let (list, error) = self.roll_back_with_tx(tx, &successor);
                rollback_list = list;
                match error {
                    None => {
                        self.stats.inc(StatType::Ledger, DetailType::Rollback);
                        debug!("Blocks rolled back: {}", rollback_list.len());
                    }
                    Some(e) => {
                        self.stats.inc(StatType::Ledger, DetailType::RollbackFailed);
                        debug!(error = ?e, "Failed to roll back");
                    }
                };
            }
        }
        rollback_list
    }

    fn block_successor_by_qualified_root(
        &self,
        tx: &dyn LedgerReadTxn,
        root: &QualifiedRoot,
    ) -> Option<BlockHash> {
        if !root.previous.is_zero() {
            self.store.successors().get(tx, &root.previous)
        } else {
            self.store
                .account()
                .get(tx, &root.root.into())
                .map(|i| i.open_block)
        }
    }

    pub fn confirm(&self, hash: BlockHash) -> Vec<SavedBlock> {
        let txn = self.store_ref().begin_write();
        let (txn, blocks) = self.confirm_max(txn, hash, 1024 * 128);
        txn.commit()
            .unwrap_or_else(|e| panic!("failed to commit confirmation txn: {e}"));
        blocks
    }

    /// Both stack and result set are bounded to limit maximum memory usage
    /// Callers must ensure that the target block was confirmed, and if not, call this function multiple times
    fn confirm_max(
        &self,
        txn: Box<dyn LedgerWriteTxn>,
        target_hash: BlockHash,
        max_blocks: usize,
    ) -> (Box<dyn LedgerWriteTxn>, Vec<SavedBlock>) {
        BlockCementer::new(self.store_ref(), &self.constants, &self.stats).confirm(
            txn,
            target_hash,
            max_blocks,
        )
    }

    pub fn confirm_batch<'a, O>(
        &self,
        batch: impl IntoIterator<Item = &'a BlockHash>,
        stopped: &AtomicBool,
        max_blocks: usize,
        cementing_observer: &mut O,
    ) where
        O: CementingObserver,
    {
        let mut confirmed = Vec::new();
        let batch: Vec<BlockHash> = batch.into_iter().cloned().collect();
        self.tx_optimistic_process(
            WriterType::ConfirmationHeight,
            DEFAULT_OPTIMISTIC_RETRIES,
            |txn, _deferred| {
                self.confirm_batch_on_txn(txn, &batch, stopped, max_blocks, cementing_observer, &mut confirmed);
                Ok(((), Vec::new()))
            },
        )
        .unwrap_or_else(|e| panic!("failed to confirm batch: {e}"));

        if !confirmed.is_empty() {
            cementing_observer.batch_confirmed(confirmed);
        }
    }

    fn confirm_batch_on_txn<O>(
        &self,
        txn: &mut dyn LedgerWriteTxn,
        batch: &[BlockHash],
        stopped: &AtomicBool,
        max_blocks: usize,
        cementing_observer: &mut O,
        confirmed: &mut Vec<(SavedBlock, BlockHash)>,
    ) where
        O: CementingObserver,
    {
        let mut blocks_confirmed = 0usize;

        for confirmation_root in batch {
            let mut success = false;
            loop {
                if stopped.load(Ordering::Relaxed) {
                    return;
                }

                if blocks_confirmed >= max_blocks {
                    self.stats
                        .inc(StatType::ConfirmingSet, DetailType::NotifyIntermediate);
                    blocks_confirmed = 0;
                    cementing_observer.batch_confirmed(std::mem::take(confirmed));
                }

                self.stats
                    .inc(StatType::ConfirmingSet, DetailType::Cementing);

                if !self.store.block().exists(txn, confirmation_root) {
                    self.stats
                        .inc(StatType::ConfirmingSet, DetailType::MissingBlock);
                    break;
                }

                let added =
                    BlockCementer::new(self.store_ref(), &self.constants, &self.stats)
                        .confirm_with_txn(txn, *confirmation_root, max_blocks);

                if !added.is_empty() {
                    self.stats.add(
                        StatType::ConfirmingSet,
                        DetailType::Cemented,
                        added.len() as u64,
                    );
                    blocks_confirmed += added.len();
                    for block in added {
                        confirmed.push((block, *confirmation_root));
                    }
                } else if BorrowingConfirmedSet::new(self.store_ref(), txn).block_exists(confirmation_root) {
                    self.stats
                        .inc(StatType::ConfirmingSet, DetailType::AlreadyCemented);
                    cementing_observer.already_confirmed(confirmation_root);
                }

                success = {
                    if let Some(block) = self.store.block().get(txn, confirmation_root) {
                        if let Some(conf_info) =
                            self.store.confirmation_height().get(txn, &block.account())
                        {
                            block.height() <= conf_info.height
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };

                if success || txn.is_refresh_needed() {
                    break;
                }
            }

            if success {
                self.stats
                    .inc(StatType::ConfirmingSet, DetailType::CementedHash);
            } else {
                self.stats
                    .inc(StatType::ConfirmingSet, DetailType::CementingFailed);
                cementing_observer.cementing_failed(confirmation_root);
            }
        }
    }

    pub fn verify_votes(
        &self,
        candidates: VecDeque<(Root, BlockHash)>,
        is_final: bool,
    ) -> VecDeque<(Root, BlockHash)> {
        let verifier = VoteVerifier {
            constants: &self.constants,
            store: self.store_ref(),
        };
        verifier.verify_votes(candidates, is_final)
    }

    pub fn block_count(&self) -> u64 {
        self.store.cache().block_count.load(Ordering::SeqCst)
    }

    pub fn block_cache_inserts(&self) -> u64 {
        self.block_count_events.inserts()
    }

    pub fn block_cache_rollbacks(&self) -> u64 {
        self.block_count_events.rollbacks()
    }

    pub fn block_cache_insert_sources(&self) -> Vec<u64> {
        self.block_count_events.insert_sources()
    }

    pub fn block_cache_duplicate_inserts(&self) -> u64 {
        self.block_count_events.duplicate_inserts()
    }

    pub fn block_cache_duplicate_sources(&self) -> Vec<u64> {
        self.block_count_events.duplicate_sources()
    }

    pub fn simulate_block_count(&self, value: u64) {
        self.store
            .cache()
            .block_count
            .store(value, Ordering::SeqCst)
    }

    pub fn confirmed_count(&self) -> u64 {
        self.store.cache().confirmed_count.load(Ordering::SeqCst)
    }

    pub fn simulate_confirmed_count(&self, value: u64) {
        self.store
            .cache()
            .confirmed_count
            .store(value, Ordering::SeqCst)
    }

    pub fn record_block_insert_source(&self, source: u8, hash: BlockHash) {
        self.block_count_events.record_insert_source(source);
        let mut tracker = self.insert_tracker.lock().unwrap();
        if let Some(record) = tracker.record_insert(hash, source) {
            drop(tracker);
            self.push_duplicate_record(record);
        }
    }

    pub(crate) fn record_block_rollback_event(&self) {
        self.block_count_events.record_rollback();
    }

    pub(crate) fn record_duplicate_insert_event(&self) {
        self.block_count_events.record_duplicate_insert();
    }

    pub fn record_duplicate_insert_source(&self, source: u8, hash: BlockHash) {
        self.block_count_events.record_duplicate_source(source);
        let tracker = self.insert_tracker.lock().unwrap();
        if let Some(record) = tracker.check_existing(&hash, source) {
            drop(tracker);
            self.push_duplicate_record(record);
        }
    }

    fn push_duplicate_record(&self, record: DuplicateInsertRecord) {
        let mut log = self.duplicate_log.lock().unwrap();
        if log.len() == MAX_DUPLICATE_LOG {
            log.pop_front();
        }
        log.push_back(record);
    }

    pub fn duplicate_insert_log(&self) -> Vec<DuplicateInsertRecordSnapshot> {
        let log = self.duplicate_log.lock().unwrap();
        log.iter()
            .rev()
            .map(|record| DuplicateInsertRecordSnapshot {
                hash: record.hash,
                first_source: record.first_source,
                second_source: record.second_source,
                timestamp: record.timestamp,
            })
            .collect()
    }

    pub fn account_count(&self) -> u64 {
        self.store.cache().account_count.load(Ordering::SeqCst)
    }

    pub fn backlog_count(&self) -> u64 {
        let blocks = self.block_count();
        let confirmed = self.confirmed_count();
        if blocks > confirmed {
            blocks - confirmed
        } else {
            0
        }
    }

    pub fn genesis(&self) -> &SavedBlock {
        &self.constants.genesis_block
    }

    pub fn work_thresholds(&self) -> &WorkThresholds {
        &self.constants.work
    }

    pub fn version(&self) -> u32 {
        let tx = self.store.begin_read();
        self.store.version().get(tx.as_ref()).unwrap_or_default() as u32
    }

    pub fn store_vendor(&self) -> String {
        self.store.vendor().to_string()
    }

    pub fn memory_stats(&self) -> anyhow::Result<MemoryStats> {
        self.store.memory_stats()
    }

    #[cfg(feature = "ledger_snapshots")]
    pub fn mark_fork(&self, root: &QualifiedRoot, snapshot_number: SnapshotNumber) {
        let mut tx = self.store_ref().begin_write();
        self.store.forks().put(tx.as_mut(), root, snapshot_number);
        tx.commit()
            .unwrap_or_else(|e| panic!("failed to commit fork marker: {e}"));
    }

    #[cfg(feature = "ledger_snapshots")]
    pub fn roll_back_forks_older_than(&self, snapshot_number: SnapshotNumber) {
        use tracing::warn;

        let forks_to_roll_back = self.find_forks_to_roll_back(snapshot_number);

        warn!("Rolling back these forks:");
        for fork in &forks_to_roll_back {
            warn!("fork hash: {:?}", fork);
        }

        for (fork_hash, _) in &forks_to_roll_back {
            if let Err(e) = self.roll_back(fork_hash) {
                use tracing::warn;
                warn!("Could not roll back fork: {e:?}")
            }
        }

        let mut txn = self.store_ref().begin_write();
        for (_, root) in forks_to_roll_back {
            self.store.forks().del(txn.as_mut(), &root);
        }
        txn.commit()
            .unwrap_or_else(|e| panic!("failed to cleanup rolled back forks: {e}"));
    }

    #[cfg(feature = "ledger_snapshots")]
    fn find_forks_to_roll_back(&self, snapshot_number: u32) -> Vec<(BlockHash, QualifiedRoot)> {
        let tx = self.store.begin_read();
        let any = BorrowingAnySet {
            constants: &self.constants,
            store: self.store_ref(),
            tx: tx.as_ref(),
            metrics: self.iterator_metrics(),
        };

        self.store
            .forks()
            .iter(tx.as_ref())
            .filter_map(|(root, snap_no)| {
                if snap_no < snapshot_number {
                    use crate::AnySet;
                    any.block_successor_by_qualified_root(&root)
                        .map(|h| (h, root))
                } else {
                    None
                }
            })
            .collect()
    }
}

impl Drop for Ledger {
    fn drop(&mut self) {
        self.store.sync().expect("sync failed");
    }
}

impl ContainerInfoProvider for Ledger {
    fn container_info(&self) -> ContainerInfo {
        ContainerInfo::builder()
            .node("rep_weights", self.rep_weights.container_info())
            .finish()
    }
}

pub struct BatchProcessResult {
    pub processed: Vec<BatchProcessEntry>,
}

pub struct BatchProcessEntry {
    pub status: Result<(), BlockError>,
    pub saved_block: Option<SavedBlock>,
    pub inserted: bool,
    pub preexisting: bool,
}

pub trait CementingObserver {
    fn already_confirmed(&mut self, hash: &BlockHash);
    fn cementing_failed(&mut self, hash: &BlockHash);
    fn batch_confirmed(&mut self, batch: Vec<(SavedBlock, BlockHash)>);
}

#[derive(Clone, Default)]
pub struct RollbackResults(Vec<RollbackResult>);

impl Deref for RollbackResults {
    type Target = Vec<RollbackResult>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for RollbackResults {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl RollbackResults {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn affected_accounts(&self) -> impl Iterator<Item = Account> + use<'_> {
        self.iter().flat_map(|i| i.affected_accounts())
    }

    pub fn hashes(&self) -> impl Iterator<Item = BlockHash> + use<'_> {
        self.iter().flat_map(|i| i.hashes())
    }

    pub fn roots(&self) -> impl Iterator<Item = Root> + use<'_> {
        self.iter().flat_map(|i| i.roots())
    }
}

#[derive(Clone)]
pub struct RollbackResult {
    pub target_hash: BlockHash,
    pub target_root: QualifiedRoot,
    pub rolled_back: Vec<SavedBlock>,
    pub error: Option<RollbackError>,
}

impl RollbackResult {
    pub fn affected_accounts(&self) -> impl Iterator<Item = Account> + use<'_> {
        self.rolled_back.iter().map(|b| b.account())
    }

    pub fn hashes(&self) -> impl Iterator<Item = BlockHash> + use<'_> {
        self.rolled_back.iter().map(|b| b.hash())
    }

    pub fn roots(&self) -> impl Iterator<Item = Root> + use<'_> {
        self.rolled_back.iter().map(|b| b.root())
    }
}
