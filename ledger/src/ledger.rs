use std::{
    collections::VecDeque,
    net::SocketAddrV6,
    ops::{Deref, DerefMut},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

use tracing::{debug, info, warn};

use rsnano_nullable_lmdb::{LmdbEnvironment, Transaction, WriteTransaction};
#[cfg(feature = "ledger_snapshots")]
use rsnano_store_lmdb::forks_store::ConfiguredForksDatabaseBuilder;
use rsnano_store_lmdb::{
    ConfiguredAccountDatabaseBuilder, ConfiguredBlockDatabaseBuilder,
    ConfiguredConfirmationHeightDatabaseBuilder, ConfiguredPeersDatabaseBuilder,
    ConfiguredPendingDatabaseBuilder, ConfiguredRepWeightDatabaseBuilder, LedgerCache, LmdbStore,
    MemoryStats,
};
#[cfg(feature = "ledger_snapshots")]
use rsnano_types::SnapshotNumber;
use rsnano_types::{
    Account, AccountInfo, Amount, Block, BlockHash, BlockPriority, ConfirmationHeightInfo, Epoch,
    Link, PendingInfo, PendingKey, PublicKey, QualifiedRoot, Root, SavedBlock, UnixTimestamp,
};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{DetailType, StatType, Stats},
};
use rsnano_work::WorkThresholds;

use crate::{
    BlockRollbackPerformer, BlockSource, BootstrapWeights, BorrowingAnySet, BorrowingConfirmedSet,
    LedgerConstants, LedgerEvent, LedgerSet, OwningAnySet, OwningConfirmedSet,
    OwningUnconfirmedSet, ProcessResult, RepWeightCache, RepWeightsUpdater, RollbackError,
    block_cementer::BlockCementer,
    block_insertion::{BlockInserter, BlockValidatorFactory},
    vote_verifier::VoteVerifier,
};
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};

type BatchValidationInput<'a> = (
    crate::block_insertion::BlockValidator<'a>,
    &'a Block,
    BlockSource,
);

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum BlockError {
    /// Signature was bad, forged or transmission error
    BadSignature,
    /// Already seen and was valid
    Old(SavedBlock),
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
            BlockError::Old(_) => "Old",
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

pub struct Ledger {
    pub store: LmdbStore,
    pub rep_weights_updater: RepWeightsUpdater,
    pub rep_weights: Arc<RepWeightCache>,
    pub constants: LedgerConstants,
    pub(crate) stats: Arc<Stats>,
    batch_validation_threads: usize,
    rollback_listener: OutputListenerMt<BlockHash>,
    store_version: u32,
    pub(crate) publish: RwLock<Option<Box<dyn Fn(LedgerEvent) + Send + Sync>>>,
    can_roll_back: RwLock<Box<dyn Fn(&BlockHash) -> bool + Send + Sync>>,
}

pub struct NullLedgerBuilder {
    blocks: ConfiguredBlockDatabaseBuilder,
    accounts: ConfiguredAccountDatabaseBuilder,
    pending: ConfiguredPendingDatabaseBuilder,
    peers: ConfiguredPeersDatabaseBuilder,
    rep_weights: ConfiguredRepWeightDatabaseBuilder,
    #[cfg(feature = "ledger_snapshots")]
    forks: ConfiguredForksDatabaseBuilder,
    confirmation_height: ConfiguredConfirmationHeightDatabaseBuilder,
    min_rep_weight: Amount,
    bootstrap_weights_max_blocks: u64,
}

impl NullLedgerBuilder {
    fn new() -> Self {
        Self {
            blocks: ConfiguredBlockDatabaseBuilder::new(),
            accounts: ConfiguredAccountDatabaseBuilder::new(),
            pending: ConfiguredPendingDatabaseBuilder::new(),
            peers: ConfiguredPeersDatabaseBuilder::new(),
            rep_weights: ConfiguredRepWeightDatabaseBuilder::new(),
            #[cfg(feature = "ledger_snapshots")]
            forks: ConfiguredForksDatabaseBuilder::new(),
            confirmation_height: ConfiguredConfirmationHeightDatabaseBuilder::new(),
            min_rep_weight: Amount::ZERO,
            bootstrap_weights_max_blocks: 0,
        }
    }

    pub fn block(mut self, block: &SavedBlock) -> Self {
        self.blocks = self.blocks.block(block);
        self
    }

    pub fn blocks<'a>(mut self, blocks: impl IntoIterator<Item = &'a SavedBlock>) -> Self {
        for b in blocks.into_iter() {
            self.blocks = self.blocks.block(b);
        }
        self
    }

    pub fn peers(mut self, peers: impl IntoIterator<Item = (SocketAddrV6, SystemTime)>) -> Self {
        for (peer, time) in peers.into_iter() {
            self.peers = self.peers.peer(peer, time)
        }
        self
    }

    pub fn rep_weights(mut self, weights: impl IntoIterator<Item = (PublicKey, Amount)>) -> Self {
        for (rep, weight) in weights.into_iter() {
            self.rep_weights = self.rep_weights.entry(rep, weight);
        }
        self
    }

    pub fn confirmation_height(mut self, account: &Account, info: &ConfirmationHeightInfo) -> Self {
        self.confirmation_height = self.confirmation_height.height(account, info);
        self
    }

    pub fn account_info(mut self, account: &Account, info: &AccountInfo) -> Self {
        self.accounts = self.accounts.account(account, info);
        self
    }

    pub fn pending(mut self, key: &PendingKey, info: &PendingInfo) -> Self {
        self.pending = self.pending.pending(key, info);
        self
    }

    #[cfg(feature = "ledger_snapshots")]
    pub fn fork(mut self, root: &QualifiedRoot, snapshot_number: SnapshotNumber) -> Self {
        self.forks = self.forks.fork(root, snapshot_number);
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

    pub fn bootstrap_weights_max_blocks(mut self, max_blocks: u64) -> Self {
        self.bootstrap_weights_max_blocks = max_blocks;
        self
    }

    pub fn finish(self) -> Ledger {
        let (block_index, block_data) = self.blocks.build();
        let env_builder = LmdbEnvironment::null_builder()
            .configured_database(block_index)
            .configured_database(block_data)
            .configured_database(self.accounts.build())
            .configured_database(self.pending.build())
            .configured_database(self.confirmation_height.build())
            .configured_database(self.peers.build())
            .configured_database(self.rep_weights.build());

        let env_builder = {
            #[cfg(not(feature = "ledger_snapshots"))]
            {
                env_builder
            }
            #[cfg(feature = "ledger_snapshots")]
            {
                env_builder.configured_database(self.forks.build())
            }
        };
        let env = env_builder.build();
        let ledger_cache = Arc::new(LedgerCache::new());
        let weights = BootstrapWeights {
            weights: Default::default(),
            max_blocks: self.bootstrap_weights_max_blocks,
        };
        let rep_weights_cache =
            RepWeightCache::with_bootstrap_weights(weights, ledger_cache, self.min_rep_weight);
        Ledger::new(
            env,
            LedgerConstants::unit_test(),
            rep_weights_cache.into(),
            Stats::default().into(),
            1,
            1,
            false,
        )
        .unwrap()
    }
}

impl Ledger {
    pub fn new_null() -> Self {
        Self::new(
            LmdbEnvironment::new_null(),
            LedgerConstants::unit_test(),
            Arc::new(RepWeightCache::default()),
            Arc::new(Stats::default()),
            1,
            1,
            false,
        )
        .unwrap()
    }

    pub fn new_null_builder() -> NullLedgerBuilder {
        NullLedgerBuilder::new()
    }

    pub(crate) fn new(
        env: LmdbEnvironment,
        constants: LedgerConstants,
        rep_weights: Arc<RepWeightCache>,
        stats: Arc<Stats>,
        thread_count: usize,
        batch_validation_threads: usize,
        consistency_check: bool,
    ) -> anyhow::Result<Self> {
        let mut store = LmdbStore::new(env)?;
        store.cache = rep_weights.ledger_cache.clone();

        let rep_weights_updater = RepWeightsUpdater::new(store.rep_weight.clone(), &rep_weights);

        let mut ledger = Self {
            rep_weights,
            rep_weights_updater,
            store,
            constants,
            stats,
            batch_validation_threads: batch_validation_threads.max(1),
            rollback_listener: Default::default(),
            store_version: 0,
            publish: RwLock::new(None),
            can_roll_back: RwLock::new(Box::new(|_| true)),
        };

        ledger.initialize(thread_count, consistency_check)?;

        Ok(ledger)
    }

    fn initialize(&mut self, thread_count: usize, consistency_check: bool) -> anyhow::Result<()> {
        {
            let txn = self.store.begin_read();
            self.store_version = self.store.version.get(&txn).unwrap_or_default() as u32;

            // Add genesis block to new ledger
            if self.store.account.iter(&txn).next().is_none() {
                let mut txn = self.store.begin_write();
                self.add_genesis_block(&mut txn);
                txn.commit();
            }
        }

        info!("Generating representative weights cache...");
        let mut total_committed_rep_weight = Amount::ZERO;
        {
            let txn = self.store.begin_read();
            let rep_weights = self.rep_weights.inner();
            let mut write_guard = rep_weights.write().unwrap();
            for (rep, weight) in self.store.rep_weight.iter(&txn) {
                write_guard.put(rep, weight);
                total_committed_rep_weight = total_committed_rep_weight
                    .checked_add(weight)
                    .expect("total rep weight should never overflow");
            }
        }
        info!("Representative weights cache generated");

        info!("Generating block and account count cache...");
        let total_account_balances = Mutex::new(Amount::ZERO);
        self.store
            .account
            .for_each_par(&self.store.env, thread_count, |iter| {
                let mut block_count = 0;
                let mut account_count = 0;
                let mut total = 0u128;

                for (_, info) in iter {
                    block_count += info.block_count;
                    account_count += 1;
                    total = total
                        .checked_add(info.balance.number())
                        .expect("total account balances should never overflow");
                }

                self.store
                    .cache
                    .block_count
                    .fetch_add(block_count, Ordering::SeqCst);

                self.store
                    .cache
                    .account_count
                    .fetch_add(account_count, Ordering::SeqCst);

                let mut guard = total_account_balances.lock().unwrap();
                *guard = guard
                    .checked_add(total.into())
                    .expect("total account balances should never overflow");
            });
        info!("Block and account count cache generated");

        info!("Generating cemented count cache...");
        self.store
            .confirmation_height
            .for_each_par(&self.store.env, thread_count, |iter| {
                let mut confirmed_count = 0;
                for (_, info) in iter {
                    confirmed_count += info.height;
                }
                self.store
                    .cache
                    .confirmed_count
                    .fetch_add(confirmed_count, Ordering::SeqCst);
            });
        info!("Cemented count cache generated");

        // Count pending balances
        if consistency_check {
            info!("Verifying ledger balance consistency...");
            let mut total_pending = Amount::ZERO;
            let txn = self.store.begin_read();
            for (_, info) in self.store.pending.iter(&txn) {
                total_pending = total_pending
                    .checked_add(info.amount)
                    .expect("total pending should never overflow");
            }

            let total_account_balances = *total_account_balances.lock().unwrap();

            assert_eq!(
                total_committed_rep_weight, total_account_balances,
                "the representative weights are inconsistent with the current account states!"
            );

            assert_eq!(
                total_account_balances.wrapping_add(total_pending),
                Amount::MAX,
                "account balances and pending balances don't add up to max supply!"
            );
            info!("Ledger balance consistency verified");
        } else {
            warn!(
                "Ledger consistency check skipped; ensure your environment provides data-integrity safeguards"
            );
        }

        Ok(())
    }

    fn add_genesis_block(&self, txn: &mut WriteTransaction) {
        let genesis_hash = self.constants.genesis_block.hash();
        let genesis_account = self.constants.genesis_account;
        self.store.block.put(txn, &self.constants.genesis_block);

        self.store.confirmation_height.put(
            txn,
            &genesis_account,
            &ConfirmationHeightInfo::new(1, genesis_hash),
        );

        self.store.account.put(
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
            .rep_weight
            .put(txn, genesis_account.into(), Amount::MAX);
    }

    pub fn any(&self) -> OwningAnySet<'_> {
        OwningAnySet::new(&self.store, &self.constants)
    }

    pub fn confirmed(&self) -> OwningConfirmedSet<'_> {
        let tx = self.store.begin_read();
        OwningConfirmedSet::new(&self.store, tx)
    }

    pub fn unconfirmed(&self) -> impl LedgerSet + use<'_> {
        let tx = self.store.begin_read();
        OwningUnconfirmedSet::new(&self.store, tx)
    }

    pub fn bootstrap_weights_max_blocks(&self) -> u64 {
        self.rep_weights.bootstrap_weights_max_blocks()
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
        txn: &mut WriteTransaction,
        account: &Account,
        old_info: &AccountInfo,
        new_info: &AccountInfo,
    ) {
        if !new_info.head.is_zero() {
            if old_info.head.is_zero() && new_info.open_block == new_info.head {
                self.store
                    .cache
                    .account_count
                    .fetch_add(1, Ordering::SeqCst);
            }
            if !old_info.head.is_zero() && old_info.epoch != new_info.epoch {
                // store.account ().put won't erase existing entries if they're in different tables
                self.store.account.del(txn, account);
            }
            self.store.account.put(txn, account, new_info);
        } else {
            debug_assert!(!self.store.confirmation_height.exists(txn, account));
            self.store.account.del(txn, account);
            debug_assert!(self.store.cache.account_count.load(Ordering::SeqCst) > 0);
            self.store
                .cache
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
        let result = self.roll_back_batch(&[*block], usize::MAX);
        let rolled_back = result[0].rolled_back.len();
        result[0].error.map_or(Ok(rolled_back), Err)
    }

    pub fn roll_back_batch<'a, T>(&self, targets: T, max_rollbacks: usize) -> RollbackResults
    where
        T: IntoIterator<Item = &'a BlockHash>,
    {
        self.stats
            .inc(StatType::BoundedBacklog, DetailType::PerformingRollbacks);

        let mut rolled_back_count = 0;
        let mut results = RollbackResults::new();
        {
            let mut txn = self.store.begin_write();
            let can_roll_back = self.can_roll_back.read().unwrap();

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
                if let Some(block) = self.store.block.get(&txn, hash) {
                    debug!(
                        "Rolling back: {}, account: {}",
                        hash,
                        block.account().encode_account()
                    );

                    let (rollback_list, error) = self.roll_back_with_txn(&mut txn, &block.hash());
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
            txn.commit();
        }

        self.notify(LedgerEvent::BlocksRolledBack(results.clone()));

        results
    }

    fn roll_back_with_txn(
        &self,
        txn: &mut WriteTransaction,
        block: &BlockHash,
    ) -> (Vec<SavedBlock>, Option<RollbackError>) {
        let mut performer = BlockRollbackPerformer::new(self, txn);
        match performer.roll_back(block) {
            Ok(()) => (performer.rolled_back, None),
            Err(e) => (performer.rolled_back, Some(e)),
        }
    }

    pub fn process_one(&self, block: &Block) -> Result<SavedBlock, BlockError> {
        let mut result = self.process_batch(std::iter::once((block, BlockSource::Local)));
        let result = result.pop().expect("should always return one result");
        match result.status {
            Ok(()) => Ok(result
                .saved_block
                .expect("saved block should always be set if block was processed")),
            Err(e) => Err(e),
        }
    }

    pub fn process_batch<'a>(
        &self,
        batch: impl IntoIterator<Item = (&'a Block, BlockSource)>,
    ) -> Vec<ProcessResult> {
        let mut validation_inputs = Vec::new();

        // Validate blocks
        {
            let tx = self.store.begin_read();
            for (block, source) in batch.into_iter() {
                let any = BorrowingAnySet {
                    constants: &self.constants,
                    store: &self.store,
                    tx: &tx,
                };
                let validator =
                    BlockValidatorFactory::new(&any, &self.constants, block).create_validator();
                validation_inputs.push((validator, block, source));
            }
        }

        let validation_results = validate_batch(validation_inputs, self.batch_validation_threads);

        // Insert blocks
        let mut processed = Vec::with_capacity(validation_results.len());
        {
            let mut txn = self.store.begin_write();
            for (result, block, source) in validation_results {
                match result {
                    Ok(instructions) => {
                        if let Some((saved_block, priority)) =
                            BlockInserter::new(self, &mut txn, block, &instructions).insert()
                        {
                            processed.push(ProcessResult {
                                block: block.clone(),
                                source,
                                status: Ok(()),
                                saved_block: Some(saved_block),
                                priority,
                            });
                        } else {
                            let err = BlockError::Conflict;
                            processed.push(ProcessResult {
                                block: block.clone(),
                                source,
                                status: Err(err),
                                saved_block: None,
                                priority: BlockPriority::default(),
                            });
                        }
                    }
                    Err(err) => {
                        processed.push(ProcessResult {
                            block: block.clone(),
                            source,
                            status: Err(err),
                            saved_block: None,
                            priority: BlockPriority::default(),
                        });
                    }
                }
            }
            txn.commit();
        }

        if !processed.is_empty() {
            self.notify(LedgerEvent::BlocksProcessed(processed.clone()));
        }

        processed
    }

    pub fn roll_back_competitors<'a, T>(&self, blocks: T)
    where
        T: IntoIterator<Item = &'a Block>,
    {
        let mut rolled_back = RollbackResults::new();
        {
            let mut txn = self.store.begin_write();
            for block in blocks {
                if txn.is_refresh_needed() {
                    txn.commit();
                    if !rolled_back.is_empty() {
                        self.notify(LedgerEvent::BlocksRolledBack(rolled_back));
                        rolled_back = RollbackResults::new();
                    }
                    txn = self.store.begin_write();
                }
                let rolled_back_blocks = self.rollback_competitor(&mut txn, block);
                if !rolled_back_blocks.is_empty() {
                    rolled_back.push(RollbackResult {
                        target_hash: block.hash(),
                        target_root: block.qualified_root(),
                        rolled_back: rolled_back_blocks,
                        error: None,
                    });
                }
            }
            txn.commit();
        }
        if !rolled_back.is_empty() {
            self.notify(LedgerEvent::BlocksRolledBack(rolled_back));
        }
    }

    fn rollback_competitor(
        &self,
        tx: &mut WriteTransaction,
        fork_block: &Block,
    ) -> Vec<SavedBlock> {
        let mut rollback_list = Vec::new();
        let hash = fork_block.hash();
        if let Some(successor) =
            self.block_successor_by_qualified_root(tx, &fork_block.qualified_root())
            && successor != hash
        {
            // Replace our block with the winner and roll back any dependent blocks
            debug!("Rolling back: {} and replacing with: {}", successor, hash);
            let (list, error) = self.roll_back_with_txn(tx, &successor);
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

        rollback_list
    }

    fn block_successor_by_qualified_root(
        &self,
        txn: &dyn Transaction,
        root: &QualifiedRoot,
    ) -> Option<BlockHash> {
        if !root.previous.is_zero() {
            self.store.successors.get(txn, &root.previous)
        } else {
            self.store
                .account
                .get(txn, &root.root.into())
                .map(|i| i.open_block)
        }
    }

    pub fn confirm(&self, hash: BlockHash) -> Vec<SavedBlock> {
        let txn = self.store.begin_write();
        let (txn, blocks) = self.confirm_max(txn, hash, 1024 * 128);
        txn.commit();
        blocks
    }

    /// Both stack and result set are bounded to limit maximum memory usage
    /// Callers must ensure that the target block was confirmed, and if not, call this function multiple times
    fn confirm_max(
        &self,
        txn: WriteTransaction,
        target_hash: BlockHash,
        max_blocks: usize,
    ) -> (WriteTransaction, Vec<SavedBlock>) {
        BlockCementer::new(&self.store, &self.constants, &self.stats).confirm(
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
        let mut blocks_confirmed = 0;
        {
            let mut txn = self.store.begin_write();

            for confirmation_root in batch.into_iter() {
                let mut success = false;
                loop {
                    if txn.is_refresh_needed() {
                        txn = self.store.env.refresh(txn);
                    }

                    // Cementing deep dependency chains might take a long time, allow for graceful shutdown, ignore notifications
                    if stopped.load(Ordering::Relaxed) {
                        txn.commit();
                        return;
                    }

                    // Issue notifications here, so that `confirmed` set is not too large before we add more blocks
                    if blocks_confirmed >= max_blocks {
                        txn.commit();
                        blocks_confirmed = 0;
                        self.stats
                            .inc(StatType::ConfirmingSet, DetailType::NotifyIntermediate);
                        self.notify(LedgerEvent::BlocksConfirmed(confirmed));
                        confirmed = Vec::new();
                        txn = self.store.env.begin_write();
                    }

                    self.stats
                        .inc(StatType::ConfirmingSet, DetailType::Cementing);

                    // The block might be rolled back before it's fully confirmed
                    if !self.store.block.exists(&txn, confirmation_root) {
                        self.stats
                            .inc(StatType::ConfirmingSet, DetailType::MissingBlock);
                        break;
                    }

                    let (t, added) = self.confirm_max(txn, *confirmation_root, max_blocks);
                    txn = t;

                    if !added.is_empty() {
                        // Confirming this block may implicitly confirm more
                        self.stats.add(
                            StatType::ConfirmingSet,
                            DetailType::Cemented,
                            added.len() as u64,
                        );
                        blocks_confirmed += added.len();
                        for block in added {
                            confirmed.push((block, *confirmation_root));
                        }
                    } else if BorrowingConfirmedSet::new(&self.store, &txn)
                        .block_exists(confirmation_root)
                    {
                        self.stats
                            .inc(StatType::ConfirmingSet, DetailType::AlreadyCemented);
                        cementing_observer.already_confirmed(confirmation_root);
                    }

                    success = {
                        if let Some(block) = self.store.block.get(&txn, confirmation_root) {
                            if let Some(conf_info) =
                                self.store.confirmation_height.get(&txn, &block.account())
                            {
                                block.height() <= conf_info.height
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    };

                    if success {
                        break;
                    }
                }

                if success {
                    self.stats
                        .inc(StatType::ConfirmingSet, DetailType::CementedHash);
                } else {
                    self.stats
                        .inc(StatType::ConfirmingSet, DetailType::CementingFailed);

                    // Requeue failed blocks for processing later
                    // Add them to the deferred set while still holding the exclusive database write transaction to avoid block processor races
                    cementing_observer.cementing_failed(confirmation_root);
                }
            }
            txn.commit();
        }

        if !confirmed.is_empty() {
            self.notify(LedgerEvent::BlocksConfirmed(confirmed));
        }
    }

    pub fn verify_votes(
        &self,
        candidates: VecDeque<(Root, BlockHash)>,
        is_final: bool,
    ) -> VecDeque<(Root, BlockHash)> {
        let verifier = VoteVerifier {
            constants: &self.constants,
            store: &self.store,
        };
        verifier.verify_votes(candidates, is_final)
    }

    pub fn block_count(&self) -> u64 {
        self.store.cache.block_count.load(Ordering::SeqCst)
    }

    pub fn simulate_block_count(&self, value: u64) {
        self.store.cache.block_count.store(value, Ordering::SeqCst)
    }

    pub fn confirmed_count(&self) -> u64 {
        self.store.cache.confirmed_count.load(Ordering::SeqCst)
    }

    pub fn simulate_confirmed_count(&self, value: u64) {
        self.store
            .cache
            .confirmed_count
            .store(value, Ordering::SeqCst)
    }

    pub fn account_count(&self) -> u64 {
        self.store.cache.account_count.load(Ordering::SeqCst)
    }

    pub fn backlog_size(&self) -> u64 {
        let blocks = self.block_count();
        let confirmed = self.confirmed_count();
        blocks.saturating_sub(confirmed)
    }

    pub fn genesis(&self) -> &SavedBlock {
        &self.constants.genesis_block
    }

    pub fn work_thresholds(&self) -> &WorkThresholds {
        &self.constants.work
    }

    pub fn version(&self) -> u32 {
        self.store_version
    }

    pub fn store_vendor(&self) -> String {
        // hard coded version! TODO: read version from Cargo
        format!("lmdb-rkv {}.{}.{}", 0, 14, 0)
    }

    pub fn memory_stats(&self) -> anyhow::Result<MemoryStats> {
        self.store.memory_stats()
    }

    #[cfg(feature = "ledger_snapshots")]
    pub fn mark_fork(&self, root: &QualifiedRoot, snapshot_number: SnapshotNumber) {
        let mut txn = self.store.begin_write();
        self.store.forks.put(&mut txn, root, snapshot_number);
        txn.commit();
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

        let mut txn = self.store.begin_write();
        for (_, root) in forks_to_roll_back {
            self.store.forks.del(&mut txn, &root);
        }
        txn.commit();
    }

    pub fn set_can_roll_back(&self, f: impl Fn(&BlockHash) -> bool + Send + Sync + 'static) {
        *self.can_roll_back.write().unwrap() = Box::new(f);
    }

    pub fn drop_publisher(&self) {
        *self.publish.write().unwrap() = None;
    }

    #[cfg(feature = "ledger_snapshots")]
    fn find_forks_to_roll_back(&self, snapshot_number: u32) -> Vec<(BlockHash, QualifiedRoot)> {
        let txn = self.store.begin_read();
        let any = BorrowingAnySet {
            constants: &self.constants,
            store: &self.store,
            tx: &txn,
        };

        self.store
            .forks
            .iter(&txn)
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

    fn notify(&self, ev: LedgerEvent) {
        let guard = self.publish.read().unwrap();
        if let Some(callback) = &*guard {
            (callback)(ev);
        }
    }
}

impl Drop for Ledger {
    fn drop(&mut self) {
        self.store.env.sync().expect("sync failed");
    }
}

fn validate_batch<'a>(
    validation_inputs: Vec<BatchValidationInput<'a>>,
    batch_validation_threads: usize,
) -> Vec<(
    Result<crate::block_insertion::BlockInsertInstructions, BlockError>,
    &'a Block,
    BlockSource,
)> {
    const MIN_PARALLEL_VALIDATION_BATCH: usize = 128;

    if validation_inputs.len() < MIN_PARALLEL_VALIDATION_BATCH {
        return validation_inputs
            .iter()
            .map(|(validator, block, source)| (validator.validate(), *block, *source))
            .collect();
    }

    let worker_count = batch_validation_threads.max(1).min(validation_inputs.len());

    if worker_count <= 1 {
        return validation_inputs
            .iter()
            .map(|(validator, block, source)| (validator.validate(), *block, *source))
            .collect();
    }

    let chunk_size = validation_inputs.len().div_ceil(worker_count);

    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for chunk in validation_inputs.chunks(chunk_size) {
            handles.push(scope.spawn(move || {
                chunk
                    .iter()
                    .map(|(validator, block, source)| (validator.validate(), *block, *source))
                    .collect::<Vec<_>>()
            }));
        }

        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect()
    })
}

impl ContainerInfoProvider for Ledger {
    fn container_info(&self) -> ContainerInfo {
        ContainerInfo::builder()
            .node("rep_weights", self.rep_weights.container_info())
            .finish()
    }
}

pub struct BatchProcessResult {
    pub processed: Vec<(Result<(), BlockError>, Option<SavedBlock>)>,
}

pub trait CementingObserver {
    fn already_confirmed(&mut self, hash: &BlockHash);
    fn cementing_failed(&mut self, hash: &BlockHash);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BlockSource, LedgerBuilder, LedgerEvent, LedgerSet,
        test_helpers::UnsavedBlockLatticeBuilder,
    };
    use rsnano_store_lmdb::{LmdbConfig, SyncStrategy};
    use rsnano_types::{BlockHash, PrivateKey};
    use std::{
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        time::{Instant, SystemTime, UNIX_EPOCH},
    };

    static TEST_LEDGER_DIR_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn builds_nulled_ledger() {
        let ledger = Ledger::new_null_builder()
            .bootstrap_weights_max_blocks(123)
            .finish();

        assert_eq!(ledger.bootstrap_weights_max_blocks(), 123);
    }

    #[test]
    fn process_batch_reports_valid_live_blocks_and_emits_processed_event() {
        let ledger = Ledger::new_null();
        let mut lattice = UnsavedBlockLatticeBuilder::with_stub_work();
        let account1 = PrivateKey::from(1);
        let account2 = PrivateKey::from(2);
        let send1 = lattice.genesis().send(&account1, 1);
        let open1 = lattice.account(&account1).receive(&send1);
        let send2 = lattice.genesis().send(&account2, 2);
        let open2 = lattice.account(&account2).receive(&send2);
        ledger.process_one(&send1).unwrap();
        ledger.confirm(send1.hash());
        ledger.process_one(&send2).unwrap();
        ledger.confirm(send2.hash());

        let observed_batches = Arc::new(Mutex::new(Vec::new()));
        let observed_batches_l = observed_batches.clone();
        *ledger.publish.write().unwrap() = Some(Box::new(move |event| {
            if let LedgerEvent::BlocksProcessed(results) = event {
                observed_batches_l.lock().unwrap().push(
                    results
                        .iter()
                        .map(|result| (result.source, result.status.is_ok()))
                        .collect::<Vec<_>>(),
                );
            }
        }));

        let results = ledger.process_batch([
            (&open1, BlockSource::Live),
            (&open2, BlockSource::LiveOriginator),
        ]);

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result.status.is_ok()));
        assert!(results.iter().all(|result| result.saved_block.is_some()));
        assert_eq!(
            results
                .iter()
                .map(|result| result.source)
                .collect::<Vec<_>>(),
            vec![BlockSource::Live, BlockSource::LiveOriginator]
        );
        assert!(
            results
                .iter()
                .all(|result| result.priority.balance > Amount::ZERO)
        );
        assert_eq!(
            *observed_batches.lock().unwrap(),
            vec![vec![
                (BlockSource::Live, true),
                (BlockSource::LiveOriginator, true)
            ]]
        );
    }

    #[test]
    fn process_batch_parallel_validation_preserves_order_sources_and_errors() {
        let ledger = Ledger::new_null();
        let mut lattice = UnsavedBlockLatticeBuilder::with_stub_work();
        let mut blocks = Vec::new();
        let duplicate_old_index = 17;
        let gap_source_index = 73;
        let total_blocks = 130;

        for i in 0..total_blocks {
            let account = PrivateKey::from(i as u64 + 1);
            let source = if i % 2 == 0 {
                BlockSource::Live
            } else {
                BlockSource::LiveOriginator
            };

            if i == gap_source_index {
                let mut isolated_lattice = UnsavedBlockLatticeBuilder::with_stub_work();
                let send = isolated_lattice.genesis().send(&account, 1);
                let open = isolated_lattice.account(&account).receive(&send);
                blocks.push((open, source));
                continue;
            }

            let send = lattice.genesis().send(&account, 1);
            let open = lattice.account(&account).receive(&send);
            ledger.process_one(&send).unwrap();
            ledger.confirm(send.hash());

            if i == duplicate_old_index {
                ledger.process_one(&open).unwrap();
            }

            blocks.push((open, source));
        }

        let expected = blocks
            .iter()
            .map(|(block, source)| (block.hash(), *source))
            .collect::<Vec<_>>();

        let results = ledger.process_batch(blocks.iter().map(|(block, source)| (block, *source)));

        assert_eq!(results.len(), total_blocks);
        for (i, result) in results.iter().enumerate() {
            let (expected_hash, expected_source) = expected[i];
            assert_eq!(result.block.hash(), expected_hash, "block order at {i}");
            assert_eq!(result.source, expected_source, "source pairing at {i}");

            if i == duplicate_old_index {
                let Err(BlockError::Old(existing)) = &result.status else {
                    panic!("expected old block at {i}, got {:?}", result.status);
                };
                assert_eq!(existing.hash(), expected_hash);
                assert!(result.saved_block.is_none());
            } else if i == gap_source_index {
                assert_eq!(result.status, Err(BlockError::GapSource));
                assert!(result.saved_block.is_none());
            } else {
                assert_eq!(result.status, Ok(()), "valid insert at {i}");
                assert_eq!(result.saved_block.as_ref().unwrap().hash(), expected_hash);
            }
        }
    }

    #[test]
    fn real_lmdb_process_batch_reports_batch_timing_for_valid_live_blocks() {
        let test_dir = TestLedgerDir::new();
        let ledger = create_real_lmdb_ledger(&test_dir);
        let mut lattice = UnsavedBlockLatticeBuilder::with_stub_work();
        let mut opens = Vec::new();

        for i in 0..64 {
            let account = PrivateKey::from(i + 1);
            let send = lattice.genesis().send(&account, 1);
            let open = lattice.account(&account).receive(&send);
            ledger.process_one(&send).unwrap();
            ledger.confirm(send.hash());
            opens.push(open);
        }

        let start = Instant::now();
        let results = ledger.process_batch(opens.iter().map(|block| (block, BlockSource::Live)));
        let elapsed = start.elapsed();

        assert_eq!(results.len(), opens.len());
        assert!(results.iter().all(|result| result.status.is_ok()));
        assert!(results.iter().all(|result| result.saved_block.is_some()));
        assert!(
            results
                .iter()
                .all(|result| result.source == BlockSource::Live)
        );
        assert!(elapsed.as_nanos() > 0);

        eprintln!(
            "ledger_process_batch_lmdb blocks={} elapsed_us={} blocks_per_sec={:.2}",
            results.len(),
            elapsed.as_micros(),
            results.len() as f64 / elapsed.as_secs_f64()
        );
    }

    #[test]
    fn real_lmdb_confirm_batch_reports_confirmation_timing_for_valid_live_blocks() {
        let test_dir = TestLedgerDir::new();
        let ledger = create_real_lmdb_ledger(&test_dir);
        let mut lattice = UnsavedBlockLatticeBuilder::with_stub_work();
        let mut open_hashes = Vec::new();

        for i in 0..64 {
            let account = PrivateKey::from(i + 1);
            let send = lattice.genesis().send(&account, 1);
            let open = lattice.account(&account).receive(&send);
            ledger.process_one(&send).unwrap();
            ledger.confirm(send.hash());
            let saved_open = ledger.process_one(&open).unwrap();
            open_hashes.push(saved_open.hash());
        }

        let stopped = AtomicBool::new(false);
        let mut observer = TimingCementingObserver::default();
        let start = Instant::now();
        ledger.confirm_batch(open_hashes.iter(), &stopped, 1024, &mut observer);
        let elapsed = start.elapsed();

        assert!(!stopped.load(Ordering::Relaxed));
        assert!(observer.failed.is_empty());
        assert!(observer.already_confirmed.is_empty());
        assert!(
            open_hashes
                .iter()
                .all(|hash| ledger.confirmed().block_exists(hash))
        );
        assert!(elapsed.as_nanos() > 0);

        eprintln!(
            "ledger_confirm_batch_lmdb blocks={} elapsed_us={} blocks_per_sec={:.2}",
            open_hashes.len(),
            elapsed.as_micros(),
            open_hashes.len() as f64 / elapsed.as_secs_f64()
        );
    }

    fn create_real_lmdb_ledger(test_dir: &TestLedgerDir) -> Ledger {
        LedgerBuilder::new(test_dir.ledger_path())
            .config(LmdbConfig {
                sync: SyncStrategy::NosyncUnsafe,
                map_size: 128 * 1024 * 1024,
                ..Default::default()
            })
            .constants(LedgerConstants::unit_test())
            .init_thread_count(1)
            .consistency_check(false)
            .finish()
            .unwrap()
    }

    #[derive(Default)]
    struct TimingCementingObserver {
        already_confirmed: Vec<BlockHash>,
        failed: Vec<BlockHash>,
    }

    impl CementingObserver for TimingCementingObserver {
        fn already_confirmed(&mut self, hash: &BlockHash) {
            self.already_confirmed.push(*hash);
        }

        fn cementing_failed(&mut self, hash: &BlockHash) {
            self.failed.push(*hash);
        }
    }

    struct TestLedgerDir {
        path: PathBuf,
    }

    impl TestLedgerDir {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let id = TEST_LEDGER_DIR_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "rsnano-ledger-pressure-{}-{unique}-{id}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn ledger_path(&self) -> PathBuf {
            self.path.join("data.ldb")
        }
    }

    impl Drop for TestLedgerDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
