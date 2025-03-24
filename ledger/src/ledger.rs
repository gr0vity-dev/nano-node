use crate::{
    block_cementer::BlockCementer,
    block_insertion::{BlockInserter, BlockValidatorFactory},
    vote_verifier::VoteVerifier,
    AnySet, BlockRollbackPerformer, BorrowingAnySet, ConfirmedSet, GenerateCacheFlags,
    LedgerConstants, LedgerSet, OwningAnySet, OwningConfirmedSet, OwningUnconfirmedSet,
    RepWeightCache, RepWeightsUpdater, Writer,
};
use rsnano_core::{
    utils::{BackpressureSender, ContainerInfo, UnixTimestamp},
    Account, AccountInfo, Amount, Block, BlockHash, ConfirmationHeightInfo, Epoch, Link,
    PendingInfo, PendingKey, PublicKey, QualifiedRoot, Root, SavedBlock,
};
use rsnano_stats::{DetailType, StatType, Stats};
use rsnano_store_lmdb::{
    ConfiguredAccountDatabaseBuilder, ConfiguredBlockDatabaseBuilder,
    ConfiguredConfirmationHeightDatabaseBuilder, ConfiguredPeersDatabaseBuilder,
    ConfiguredPendingDatabaseBuilder, ConfiguredPrunedDatabaseBuilder, LedgerCache,
    LmdbAccountStore, LmdbBlockStore, LmdbConfirmationHeightStore, LmdbEnv, LmdbFinalVoteStore,
    LmdbOnlineWeightStore, LmdbPeerStore, LmdbPendingStore, LmdbPrunedStore, LmdbRepWeightStore,
    LmdbStore, LmdbVersionStore, LmdbWriteTransaction, MemoryStats, Transaction, WriteQueue,
};
use rsnano_work::WorkThresholds;
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddrV6,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::SystemTime,
};
use tracing::{debug, error};

#[derive(PartialEq, Eq, Debug, Clone, Copy, FromPrimitive)]
#[repr(u8)]
pub enum BlockStatus {
    Progress,      // Hasn't been seen before, signed correctly
    BadSignature,  // Signature was bad, forged or transmission error
    Old,           // Already seen and was valid
    NegativeSpend, // Malicious attempt to spend a negative amount
    Fork,          // Malicious fork based on previous
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
}

impl BlockStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlockStatus::Progress => "Progress",
            BlockStatus::BadSignature => "Bad signature",
            BlockStatus::Old => "Old",
            BlockStatus::NegativeSpend => "Negative spend",
            BlockStatus::Fork => "Fork",
            BlockStatus::Unreceivable => "Unreceivable",
            BlockStatus::GapPrevious => "Gap previous",
            BlockStatus::GapSource => "Gap source",
            BlockStatus::GapEpochOpenPending => "Gap epoch open pendign",
            BlockStatus::OpenedBurnAccount => "Opened burn account",
            BlockStatus::BalanceMismatch => "Balance mismatch",
            BlockStatus::RepresentativeMismatch => "Representative mismatch",
            BlockStatus::BlockPosition => "Block position",
            BlockStatus::InsufficientWork => "Insufficient work",
        }
    }
}

impl From<BlockStatus> for DetailType {
    fn from(value: BlockStatus) -> Self {
        match value {
            BlockStatus::Progress => Self::Progress,
            BlockStatus::BadSignature => Self::BadSignature,
            BlockStatus::Old => Self::Old,
            BlockStatus::NegativeSpend => Self::NegativeSpend,
            BlockStatus::Fork => Self::Fork,
            BlockStatus::Unreceivable => Self::Unreceivable,
            BlockStatus::GapPrevious => Self::GapPrevious,
            BlockStatus::GapSource => Self::GapSource,
            BlockStatus::GapEpochOpenPending => Self::GapEpochOpenPending,
            BlockStatus::OpenedBurnAccount => Self::OpenedBurnAccount,
            BlockStatus::BalanceMismatch => Self::BalanceMismatch,
            BlockStatus::RepresentativeMismatch => Self::RepresentativeMismatch,
            BlockStatus::BlockPosition => Self::BlockPosition,
            BlockStatus::InsufficientWork => Self::InsufficientWork,
        }
    }
}

pub enum LedgerEvent {
    /// The confirmed block + it's confirmation root
    BlocksConfirmed(Vec<(SavedBlock, BlockHash)>),
}

pub struct Ledger {
    pub store: Arc<LmdbStore>,
    pub rep_weights_updater: RepWeightsUpdater,
    pub rep_weights: Arc<RepWeightCache>,
    pub constants: LedgerConstants,
    pruning: AtomicBool,
    pub(crate) stats: Arc<Stats>,
    event_sender: RwLock<Option<BackpressureSender<LedgerEvent>>>,
}

pub struct NullLedgerBuilder {
    blocks: ConfiguredBlockDatabaseBuilder,
    accounts: ConfiguredAccountDatabaseBuilder,
    pending: ConfiguredPendingDatabaseBuilder,
    pruned: ConfiguredPrunedDatabaseBuilder,
    peers: ConfiguredPeersDatabaseBuilder,
    confirmation_height: ConfiguredConfirmationHeightDatabaseBuilder,
    min_rep_weight: Amount,
}

impl NullLedgerBuilder {
    fn new() -> Self {
        Self {
            blocks: ConfiguredBlockDatabaseBuilder::new(),
            accounts: ConfiguredAccountDatabaseBuilder::new(),
            pending: ConfiguredPendingDatabaseBuilder::new(),
            pruned: ConfiguredPrunedDatabaseBuilder::new(),
            peers: ConfiguredPeersDatabaseBuilder::new(),
            confirmation_height: ConfiguredConfirmationHeightDatabaseBuilder::new(),
            min_rep_weight: Amount::zero(),
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

    pub fn pruned(mut self, hash: &BlockHash) -> Self {
        self.pruned = self.pruned.pruned(hash);
        self
    }

    pub fn finish(self) -> Ledger {
        let env = Arc::new(
            LmdbEnv::new_null_with()
                .configured_database(self.blocks.build())
                .configured_database(self.accounts.build())
                .configured_database(self.pending.build())
                .configured_database(self.pruned.build())
                .configured_database(self.confirmation_height.build())
                .configured_database(self.peers.build())
                .build(),
        );

        let store = LmdbStore {
            write_queue: Arc::new(WriteQueue::new()),
            cache: Arc::new(LedgerCache::new()),
            env: env.clone(),
            account: Arc::new(LmdbAccountStore::new(env.clone()).unwrap()),
            block: Arc::new(LmdbBlockStore::new(env.clone()).unwrap()),
            confirmation_height: Arc::new(LmdbConfirmationHeightStore::new(env.clone()).unwrap()),
            final_vote: Arc::new(LmdbFinalVoteStore::new(env.clone()).unwrap()),
            online_weight: Arc::new(LmdbOnlineWeightStore::new(env.clone()).unwrap()),
            peer: Arc::new(LmdbPeerStore::new(env.clone()).unwrap()),
            pending: Arc::new(LmdbPendingStore::new(env.clone()).unwrap()),
            pruned: Arc::new(LmdbPrunedStore::new(env.clone()).unwrap()),
            rep_weight: Arc::new(LmdbRepWeightStore::new(env.clone()).unwrap()),
            version: Arc::new(LmdbVersionStore::new(env.clone()).unwrap()),
        };
        Ledger::new(
            Arc::new(store),
            LedgerConstants::unit_test(),
            self.min_rep_weight,
            Arc::new(RepWeightCache::new()),
            Arc::new(Stats::default()),
        )
        .unwrap()
    }
}

impl Ledger {
    pub fn new_null() -> Self {
        Self::new(
            Arc::new(LmdbStore::new_null()),
            LedgerConstants::unit_test(),
            Amount::zero(),
            Arc::new(RepWeightCache::new()),
            Arc::new(Stats::default()),
        )
        .unwrap()
    }

    pub fn new_null_builder() -> NullLedgerBuilder {
        NullLedgerBuilder::new()
    }

    pub fn new(
        store: Arc<LmdbStore>,
        constants: LedgerConstants,
        min_rep_weight: Amount,
        rep_weights: Arc<RepWeightCache>,
        stats: Arc<Stats>,
    ) -> anyhow::Result<Self> {
        let rep_weights_updater =
            RepWeightsUpdater::new(store.rep_weight.clone(), min_rep_weight, &rep_weights);

        let mut ledger = Self {
            rep_weights,
            rep_weights_updater,
            store,
            constants,
            pruning: AtomicBool::new(false),
            stats,
            event_sender: RwLock::new(None),
        };

        ledger.initialize(&GenerateCacheFlags::new())?;

        Ok(ledger)
    }

    pub fn set_event_sink(&mut self, sink: BackpressureSender<LedgerEvent>) {
        *self.event_sender.write().unwrap() = Some(sink);
    }

    fn initialize(&mut self, generate_cache: &GenerateCacheFlags) -> anyhow::Result<()> {
        if self
            .store
            .account
            .iter(&self.store.tx_begin_read())
            .next()
            .is_none()
        {
            let mut tx = self.store.tx_begin_write(Writer::Generic);
            self.add_genesis_block(&mut tx);
        }

        if generate_cache.reps || generate_cache.account_count || generate_cache.block_count {
            self.store.account.for_each_par(|iter| {
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
                    .cache
                    .block_count
                    .fetch_add(block_count, Ordering::SeqCst);
                self.store
                    .cache
                    .account_count
                    .fetch_add(account_count, Ordering::SeqCst);
                self.rep_weights_updater.copy_from(&rep_weights);
            });
        }

        if generate_cache.confirmed_count {
            self.store.confirmation_height.for_each_par(|iter| {
                let mut confirmed_count = 0;
                for (_, info) in iter {
                    confirmed_count += info.height;
                }
                self.store
                    .cache
                    .confirmed_count
                    .fetch_add(confirmed_count, Ordering::SeqCst);
            });
        }

        let tx = self.store.tx_begin_read();
        self.store
            .cache
            .pruned_count
            .fetch_add(self.store.pruned.count(&tx), Ordering::SeqCst);

        if self.store.pruned.count(&tx) > 0 {
            self.enable_pruning();
        }

        Ok(())
    }

    fn add_genesis_block(&self, txn: &mut LmdbWriteTransaction) {
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

    pub fn any(&self) -> OwningAnySet {
        OwningAnySet::new(&self.store, &self.constants)
    }

    pub fn confirmed(&self) -> OwningConfirmedSet<'_> {
        let tx = self.store.tx_begin_read();
        OwningConfirmedSet::new(&self.store, tx)
    }

    pub fn unconfirmed(&self) -> impl LedgerSet + use<'_> {
        let tx = self.store.tx_begin_read();
        OwningUnconfirmedSet::new(&self.store, tx)
    }

    pub fn pruning_enabled(&self) -> bool {
        self.pruning.load(Ordering::SeqCst)
    }

    pub fn enable_pruning(&self) {
        self.pruning.store(true, Ordering::SeqCst);
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
        txn: &mut LmdbWriteTransaction,
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

    pub fn prune_batch(&self, targets: &mut VecDeque<BlockHash>, batch_size: usize) -> usize {
        let mut transaction_write_count = 0;
        // TODO break loop if node stopped
        if !targets.is_empty() {
            let mut tx = self.store.tx_begin_write(Writer::Pruning);
            while !targets.is_empty() && transaction_write_count < batch_size {
                let pruning_hash = targets.front().unwrap();
                let account_pruned_count =
                    self.pruning_action(&mut tx, pruning_hash, batch_size as u64);
                transaction_write_count += account_pruned_count as usize;
                targets.pop_front();
            }
        }
        transaction_write_count
    }

    pub fn prune_one(&self, target: &BlockHash, batch_size: usize) -> usize {
        let mut tx = self.store.tx_begin_write(Writer::Pruning);
        self.pruning_action(&mut tx, target, batch_size as u64) as usize
    }

    fn pruning_action(
        &self,
        txn: &mut LmdbWriteTransaction,
        hash: &BlockHash,
        batch_size: u64,
    ) -> u64 {
        self.stats.inc(StatType::Pruning, DetailType::PruningTarget);
        let mut pruned_count = 0;
        let mut hash = *hash;
        let genesis_hash = self.constants.genesis_block.hash();
        let mut any = BorrowingAnySet {
            constants: &self.constants,
            store: &self.store,
            tx: txn,
        };

        while !hash.is_zero() && hash != genesis_hash {
            if let Some(block) = any.get_block(&hash) {
                assert!(any.confirmed().block_exists_or_pruned(&hash));
                self.store.block.del(txn, &hash);
                self.store.pruned.put(txn, &hash);
                hash = block.previous();
                pruned_count += 1;
                self.store.cache.pruned_count.fetch_add(1, Ordering::SeqCst);
                if pruned_count % batch_size == 0 {
                    txn.commit();
                    txn.renew();
                }
                any = BorrowingAnySet {
                    constants: &self.constants,
                    store: &self.store,
                    tx: txn,
                };
            } else if self.store.pruned.exists(txn, &hash) {
                hash = BlockHash::zero();
            } else {
                panic!("Error finding block for pruning");
            }
        }

        self.stats
            .add(StatType::Pruning, DetailType::PrunedCount, pruned_count);

        pruned_count
    }
    ///
    /// Rollback blocks until `block' doesn't exist or it tries to penetrate the confirmation height
    pub fn rollback(
        &self,
        block: &BlockHash,
    ) -> Result<Vec<SavedBlock>, (anyhow::Error, Vec<SavedBlock>)> {
        let mut tx = self.store.tx_begin_write(Writer::BoundedBacklog);
        self.rollback_with_tx(&mut tx, block)
    }

    fn rollback_with_tx(
        &self,
        tx: &mut LmdbWriteTransaction,
        block: &BlockHash,
    ) -> Result<Vec<SavedBlock>, (anyhow::Error, Vec<SavedBlock>)> {
        let mut performer = BlockRollbackPerformer::new(self, tx);
        match performer.roll_back(block) {
            Ok(()) => Ok(performer.rolled_back),
            Err(e) => Err((e, performer.rolled_back)),
        }
    }

    pub fn rollback_batch(
        &self,
        targets: &[BlockHash],
        max_rollbacks: usize,
        can_roll_back: impl Fn(&BlockHash) -> bool,
    ) -> BatchRollbackResult {
        self.stats
            .inc(StatType::BoundedBacklog, DetailType::PerformingRollbacks);

        let mut rolled_back_count = 0;
        let mut processed = Vec::new();
        let mut processed_hashes = Vec::new();
        {
            let mut tx = self.store.tx_begin_write(Writer::BoundedBacklog);

            for hash in targets {
                // Skip the rollback if the block is being used by the node, this should be race free as it's checked while holding the ledger write lock
                if !can_roll_back(hash) {
                    self.stats
                        .inc(StatType::BoundedBacklog, DetailType::RollbackSkipped);
                    continue;
                }

                // Here we check that the block is still OK to rollback, there could be a delay between gathering the targets and performing the rollbacks
                if let Some(block) = self.store.block.get(&tx, hash) {
                    debug!(
                        "Rolling back: {}, account: {}",
                        hash,
                        block.account().encode_account()
                    );

                    let rollback_list = match self.rollback_with_tx(&mut tx, &block.hash()) {
                        Ok(rollback_list) => {
                            self.stats
                                .inc(StatType::BoundedBacklog, DetailType::Rollback);
                            rollback_list
                        }
                        Err((_, rollback_list)) => {
                            self.stats
                                .inc(StatType::BoundedBacklog, DetailType::RollbackFailed);
                            rollback_list
                        }
                    };

                    rolled_back_count += rollback_list.len();
                    for b in &rollback_list {
                        processed_hashes.push(b.hash());
                    }
                    processed.push((rollback_list, block.qualified_root()));

                    // Return early if we reached the maximum number of rollbacks
                    if rolled_back_count >= max_rollbacks {
                        break;
                    }
                } else {
                    self.stats
                        .inc(StatType::BoundedBacklog, DetailType::RollbackMissingBlock);
                    rolled_back_count += 1;
                    processed_hashes.push(*hash);
                }
            }
        }

        BatchRollbackResult {
            processed,
            processed_hashes,
        }
    }

    pub fn process_batch<'a>(
        &self,
        batch: impl IntoIterator<Item = (&'a Block, bool)>,
    ) -> BatchProcessResult {
        let mut tx = self.store.tx_begin_write(Writer::BlockProcessor);
        let mut processed = Vec::new();
        let mut rolled_back = Vec::new();

        for (block, force) in batch.into_iter() {
            tx.refresh_if_needed();

            if force {
                let rolled_back_blocks = self.rollback_competitor(&mut tx, block);
                if !rolled_back_blocks.is_empty() {
                    rolled_back.push((rolled_back_blocks, block.qualified_root()));
                }
            }

            match self.process(&mut tx, block) {
                Ok(saved_block) => {
                    processed.push((BlockStatus::Progress, Some(saved_block)));
                }
                Err(status) => {
                    processed.push((status, None));
                }
            }
        }

        BatchProcessResult {
            processed,
            rolled_back,
        }
    }

    pub fn process_one(&self, block: &Block) -> Result<SavedBlock, BlockStatus> {
        let mut tx = self.store.tx_begin_write(Writer::BlockProcessor);
        self.process(&mut tx, block)
    }

    fn process(
        &self,
        txn: &mut LmdbWriteTransaction,
        block: &Block,
    ) -> Result<SavedBlock, BlockStatus> {
        let any = BorrowingAnySet {
            constants: &self.constants,
            store: &self.store,
            tx: txn,
        };
        let validator = BlockValidatorFactory::new(&any, &self.constants, block).create_validator();
        let instructions = validator.validate()?;
        let inserted = BlockInserter::new(self, txn, block, &instructions).insert();
        Ok(inserted)
    }

    fn rollback_competitor(
        &self,
        tx: &mut LmdbWriteTransaction,
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
                rollback_list = match self.rollback_with_tx(tx, &successor) {
                    Ok(rollback_list) => {
                        self.stats.inc(StatType::Ledger, DetailType::Rollback);
                        debug!("Blocks rolled back: {}", rollback_list.len());
                        rollback_list
                    }
                    Err((e, rollback_list)) => {
                        self.stats.inc(StatType::Ledger, DetailType::RollbackFailed);
                        error!(
                            ?e,
                            "Failed to roll back: {} because it or a successor was confirmed",
                            successor
                        );
                        rollback_list
                    }
                };
            }
        }
        rollback_list
    }

    fn block_successor_by_qualified_root(
        &self,
        tx: &dyn Transaction,
        root: &QualifiedRoot,
    ) -> Option<BlockHash> {
        if !root.previous.is_zero() {
            self.store.block.successor(tx, &root.previous)
        } else {
            self.store
                .account
                .get(tx, &root.root.into())
                .map(|i| i.open_block)
        }
    }

    pub fn confirm(&self, hash: BlockHash) -> Vec<SavedBlock> {
        let mut tx = self.store.tx_begin_write(Writer::ConfirmationHeight);
        self.confirm_max(&mut tx, hash, 1024 * 128)
    }

    /// Both stack and result set are bounded to limit maximum memory usage
    /// Callers must ensure that the target block was confirmed, and if not, call this function multiple times
    fn confirm_max(
        &self,
        txn: &mut LmdbWriteTransaction,
        target_hash: BlockHash,
        max_blocks: usize,
    ) -> Vec<SavedBlock> {
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
        observer: &mut O,
    ) where
        O: CementingObserver,
    {
        let mut confirmed = Vec::new();
        let mut blocks_confirmed = 0;
        {
            let mut tx = self.store.tx_begin_write(Writer::ConfirmationHeight);

            for confirmation_root in batch.into_iter() {
                let mut success = false;
                loop {
                    tx.refresh_if_needed();

                    // Cementing deep dependency chains might take a long time, allow for graceful shutdown, ignore notifications
                    if stopped.load(Ordering::Relaxed) {
                        return;
                    }

                    // Issue notifications here, so that `confirmed` set is not too large before we add more blocks
                    if blocks_confirmed >= max_blocks {
                        tx.commit();
                        blocks_confirmed = 0;
                        self.stats
                            .inc(StatType::ConfirmingSet, DetailType::NotifyIntermediate);
                        if let Some(sender) = self.event_sender.read().unwrap().as_ref() {
                            sender
                                .send(LedgerEvent::BlocksConfirmed(confirmed))
                                .unwrap();
                        }
                        confirmed = Vec::new();
                        tx.renew();
                    }

                    self.stats
                        .inc(StatType::ConfirmingSet, DetailType::Cementing);

                    // The block might be rolled back before it's fully confirmed
                    if !self.store.block.exists(&tx, confirmation_root) {
                        self.stats
                            .inc(StatType::ConfirmingSet, DetailType::MissingBlock);
                        break;
                    }

                    let added = self.confirm_max(&mut tx, *confirmation_root, max_blocks);

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
                    } else {
                        self.stats
                            .inc(StatType::ConfirmingSet, DetailType::AlreadyCemented);
                        observer.already_confirmed(confirmation_root);
                    }

                    success = {
                        if let Some(block) = self.store.block.get(&tx, confirmation_root) {
                            if let Some(conf_info) =
                                self.store.confirmation_height.get(&tx, &block.account())
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
                    observer.cementing_failed(confirmation_root);
                }
            }
        }

        if !confirmed.is_empty() {
            if let Some(sender) = self.event_sender.read().unwrap().as_ref() {
                sender
                    .send(LedgerEvent::BlocksConfirmed(confirmed))
                    .unwrap();
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
            store: &self.store,
        };
        verifier.verify_votes(candidates, is_final)
    }

    pub fn confirmed_count(&self) -> u64 {
        self.store.cache.confirmed_count.load(Ordering::SeqCst)
    }

    pub fn block_count(&self) -> u64 {
        self.store.cache.block_count.load(Ordering::SeqCst)
    }

    pub fn account_count(&self) -> u64 {
        self.store.cache.account_count.load(Ordering::SeqCst)
    }

    pub fn pruned_count(&self) -> u64 {
        self.store.cache.pruned_count.load(Ordering::SeqCst)
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

    pub fn genesis_hash(&self) -> BlockHash {
        self.constants.genesis_block.hash()
    }

    pub fn work_thresholds(&self) -> &WorkThresholds {
        &self.constants.work
    }

    pub fn version(&self) -> u32 {
        let tx = self.store.tx_begin_read();
        self.store.version.get(&tx).unwrap_or_default() as u32
    }

    pub fn store_vendor(&self) -> String {
        self.store.vendor()
    }

    pub fn memory_stats(&self) -> anyhow::Result<MemoryStats> {
        self.store.memory_stats()
    }

    pub fn stop(&self) {
        drop(self.event_sender.write().unwrap().take())
    }

    pub fn container_info(&self) -> ContainerInfo {
        ContainerInfo::builder()
            .node("rep_weights", self.rep_weights.container_info())
            .finish()
    }
}

pub struct BatchRollbackResult {
    pub processed: Vec<(Vec<SavedBlock>, QualifiedRoot)>,
    pub processed_hashes: Vec<BlockHash>,
}

pub struct BatchProcessResult {
    pub processed: Vec<(BlockStatus, Option<SavedBlock>)>,
    pub rolled_back: Vec<(Vec<SavedBlock>, QualifiedRoot)>,
}

pub trait CementingObserver {
    fn already_confirmed(&mut self, hash: &BlockHash);
    fn cementing_failed(&mut self, hash: &BlockHash);
}
