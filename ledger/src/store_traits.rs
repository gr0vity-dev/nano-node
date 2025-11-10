use std::{
    net::SocketAddrV6,
    ops::{Bound, RangeBounds as StdRangeBounds},
    sync::Arc,
    time::SystemTime,
};

use anyhow::{Result, anyhow};
use rsnano_nullable_lmdb::{ReadTransaction, Transaction, WriteTransaction};
use rsnano_output_tracker::OutputTrackerMt;
#[cfg(feature = "ledger_snapshots")]
use rsnano_types::SnapshotNumber;
use rsnano_types::{
    Account, AccountInfo, Amount, BlockHash, ConfirmationHeightInfo, PendingInfo, PendingKey,
    PublicKey, QualifiedRoot, SavedBlock,
};

/// Simple representation of the start/end bounds used for ranged queries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RangeBounds<T> {
    pub start: Bound<T>,
    pub end: Bound<T>,
}

impl<T> RangeBounds<T> {
    pub const fn new(start: Bound<T>, end: Bound<T>) -> Self {
        Self { start, end }
    }

    pub const fn unbounded() -> Self {
        Self {
            start: Bound::Unbounded,
            end: Bound::Unbounded,
        }
    }
}

impl<T> StdRangeBounds<T> for RangeBounds<T> {
    fn start_bound(&self) -> Bound<&T> {
        match &self.start {
            Bound::Included(value) => Bound::Included(value),
            Bound::Excluded(value) => Bound::Excluded(value),
            Bound::Unbounded => Bound::Unbounded,
        }
    }

    fn end_bound(&self) -> Bound<&T> {
        match &self.end {
            Bound::Included(value) => Bound::Included(value),
            Bound::Excluded(value) => Bound::Excluded(value),
            Bound::Unbounded => Bound::Unbounded,
        }
    }
}

pub type StoreIterator<'a, T> = Box<dyn Iterator<Item = T> + 'a>;

pub trait LedgerStore: Send + Sync {
    fn block_store(&self) -> &dyn BlockStore;
    fn block(&self) -> &dyn BlockStore {
        self.block_store()
    }

    fn account_store(&self) -> &dyn AccountStore;
    fn account(&self) -> &dyn AccountStore {
        self.account_store()
    }

    fn pending_store(&self) -> &dyn PendingStore;
    fn pending(&self) -> &dyn PendingStore {
        self.pending_store()
    }

    fn confirmation_height_store(&self) -> &dyn ConfirmationHeightStore;
    fn confirmation_height(&self) -> &dyn ConfirmationHeightStore {
        self.confirmation_height_store()
    }

    fn successor_store(&self) -> &dyn SuccessorStore;
    fn successors(&self) -> &dyn SuccessorStore {
        self.successor_store()
    }

    fn final_vote_store(&self) -> &dyn FinalVoteStore;
    fn final_vote(&self) -> &dyn FinalVoteStore {
        self.final_vote_store()
    }

    fn peer_store(&self) -> &dyn PeerStore;
    fn peer(&self) -> &dyn PeerStore {
        self.peer_store()
    }

    fn version_store(&self) -> &dyn VersionStore;
    fn version(&self) -> &dyn VersionStore {
        self.version_store()
    }

    fn online_weight_store(&self) -> &dyn OnlineWeightStore;
    fn online_weight(&self) -> &dyn OnlineWeightStore {
        self.online_weight_store()
    }

    fn rep_weight_store(&self) -> Arc<dyn RepWeightStore>;
    fn rep_weight(&self) -> Arc<dyn RepWeightStore> {
        self.rep_weight_store()
    }

    #[cfg(feature = "ledger_snapshots")]
    fn forks_store(&self) -> &dyn ForksStore;
    #[cfg(feature = "ledger_snapshots")]
    fn forks(&self) -> &dyn ForksStore {
        self.forks_store()
    }

    fn begin_read(&self) -> ReadTransaction;
    fn begin_write(&self) -> WriteTransaction;
    fn refresh_write_txn(&self, txn: WriteTransaction) -> WriteTransaction;
    fn sync(&self) -> Result<()>;
    fn cache(&self) -> &LedgerCache;
    fn memory_stats(&self) -> Result<MemoryStats>;

    fn for_each_account_par(
        &self,
        thread_count: usize,
        action: &(dyn Fn(&mut dyn Iterator<Item = (Account, AccountInfo)>) + Send + Sync),
    );

    fn for_each_confirmation_height_par(
        &self,
        thread_count: usize,
        action: &(dyn Fn(&mut dyn Iterator<Item = (Account, ConfirmationHeightInfo)>) + Send + Sync),
    );
}

pub trait BlockStore: Send + Sync {
    fn put(&self, txn: &mut WriteTransaction, block: &SavedBlock);
    fn get(&self, txn: &dyn Transaction, hash: &BlockHash) -> Option<SavedBlock>;
    fn del(&self, txn: &mut WriteTransaction, hash: &BlockHash);
    fn exists(&self, txn: &dyn Transaction, hash: &BlockHash) -> bool;
    fn iter<'a>(&'a self, txn: &'a dyn Transaction) -> StoreIterator<'a, SavedBlock>;
    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn Transaction,
        range: RangeBounds<BlockHash>,
    ) -> StoreIterator<'a, SavedBlock>;
    fn track_puts(&self) -> Arc<OutputTrackerMt<SavedBlock>>;
}

pub trait AccountStore: Send + Sync {
    fn put(&self, txn: &mut WriteTransaction, account: &Account, info: &AccountInfo);
    fn get(&self, txn: &dyn Transaction, account: &Account) -> Option<AccountInfo>;
    fn del(&self, txn: &mut WriteTransaction, account: &Account);
    fn iter<'a>(&'a self, txn: &'a dyn Transaction) -> StoreIterator<'a, (Account, AccountInfo)>;
    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn Transaction,
        range: RangeBounds<Account>,
    ) -> StoreIterator<'a, (Account, AccountInfo)>;
    fn track_puts(&self) -> Arc<OutputTrackerMt<(Account, AccountInfo)>>;
}

pub trait PendingStore: Send + Sync {
    fn put(&self, txn: &mut WriteTransaction, key: &PendingKey, pending: &PendingInfo);
    fn del(&self, txn: &mut WriteTransaction, key: &PendingKey);
    fn get(&self, txn: &dyn Transaction, key: &PendingKey) -> Option<PendingInfo>;
    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn Transaction,
        range: RangeBounds<PendingKey>,
    ) -> StoreIterator<'a, (PendingKey, PendingInfo)>;
    fn track_puts(&self) -> Arc<OutputTrackerMt<(PendingKey, PendingInfo)>>;
    fn track_deletions(&self) -> Arc<OutputTrackerMt<PendingKey>>;
}

pub trait ConfirmationHeightStore: Send + Sync {
    fn put(&self, txn: &mut WriteTransaction, account: &Account, info: &ConfirmationHeightInfo);
    fn get(&self, txn: &dyn Transaction, account: &Account) -> Option<ConfirmationHeightInfo>;
    fn exists(&self, txn: &dyn Transaction, account: &Account) -> bool;
    fn iter<'a>(
        &'a self,
        txn: &'a dyn Transaction,
    ) -> StoreIterator<'a, (Account, ConfirmationHeightInfo)>;
}

pub trait RepWeightStore: Send + Sync {
    fn get(&self, txn: &dyn Transaction, rep: &PublicKey) -> Option<Amount>;
    fn put(&self, txn: &mut WriteTransaction, representative: PublicKey, weight: Amount);
    fn del(&self, txn: &mut WriteTransaction, representative: &PublicKey);
    fn track_puts(&self) -> Arc<OutputTrackerMt<(PublicKey, Amount)>>;
    fn track_deletions(&self) -> Arc<OutputTrackerMt<PublicKey>>;
}

pub trait SuccessorStore: Send + Sync {
    fn put(&self, txn: &mut WriteTransaction, block: &BlockHash, successor: &BlockHash);
    fn del(&self, txn: &mut WriteTransaction, block: &BlockHash);
    fn get(&self, txn: &dyn Transaction, block: &BlockHash) -> Option<BlockHash>;
    fn track_puts(&self) -> Arc<OutputTrackerMt<(BlockHash, BlockHash)>>;
}

pub trait FinalVoteStore: Send + Sync {
    fn put(&self, txn: &mut WriteTransaction, root: &QualifiedRoot, hash: &BlockHash) -> bool;
    fn get(&self, txn: &dyn Transaction, root: &QualifiedRoot) -> Option<BlockHash>;
}

pub trait PeerStore: Send + Sync {
    fn put(&self, txn: &mut WriteTransaction, endpoint: SocketAddrV6, time: SystemTime);
    fn del(&self, txn: &mut WriteTransaction, endpoint: SocketAddrV6);
    fn exists(&self, txn: &dyn Transaction, endpoint: SocketAddrV6) -> bool;
    fn iter<'a>(
        &'a self,
        txn: &'a dyn Transaction,
    ) -> StoreIterator<'a, (SocketAddrV6, SystemTime)>;
    fn track_puts(&self) -> Arc<OutputTrackerMt<(SocketAddrV6, SystemTime)>>;
    fn track_deletions(&self) -> Arc<OutputTrackerMt<SocketAddrV6>>;
}

pub trait OnlineWeightStore: Send + Sync {
    fn put(&self, txn: &mut WriteTransaction, time: u64, amount: &Amount);
    fn del(&self, txn: &mut WriteTransaction, time: u64);
    fn iter<'a>(&'a self, txn: &'a dyn Transaction) -> StoreIterator<'a, (u64, Amount)>;
    fn iter_rev<'a>(&'a self, txn: &'a dyn Transaction) -> StoreIterator<'a, (u64, Amount)>;
}

pub trait VersionStore: Send + Sync {
    fn get(&self, txn: &dyn Transaction) -> Option<i32>;
}

#[cfg(feature = "ledger_snapshots")]
pub trait ForksStore: Send + Sync {
    fn put(&self, txn: &mut WriteTransaction, root: &QualifiedRoot, snapshot: SnapshotNumber);
    fn del(&self, txn: &mut WriteTransaction, root: &QualifiedRoot);
    fn get(&self, txn: &dyn Transaction, root: &QualifiedRoot) -> Option<SnapshotNumber>;
    fn iter<'a>(
        &'a self,
        txn: &'a dyn Transaction,
    ) -> StoreIterator<'a, (QualifiedRoot, SnapshotNumber)>;
}

#[cfg(feature = "ledger_snapshots")]
use rsnano_store_lmdb::forks_store::LmdbForksStore;
use rsnano_store_lmdb::{
    LedgerCache, LmdbAccountStore, LmdbBlockStore, LmdbConfirmationHeightStore, LmdbFinalVoteStore,
    LmdbOnlineWeightStore, LmdbPeerStore, LmdbPendingStore, LmdbRepWeightStore, LmdbStore,
    LmdbSuccessorStore, LmdbVersionStore, MemoryStats,
};

impl LedgerStore for LmdbStore {
    fn block_store(&self) -> &dyn BlockStore {
        &self.block
    }

    fn account_store(&self) -> &dyn AccountStore {
        &self.account
    }

    fn pending_store(&self) -> &dyn PendingStore {
        &self.pending
    }

    fn confirmation_height_store(&self) -> &dyn ConfirmationHeightStore {
        &self.confirmation_height
    }

    fn successor_store(&self) -> &dyn SuccessorStore {
        &self.successors
    }

    fn final_vote_store(&self) -> &dyn FinalVoteStore {
        &self.final_vote
    }

    fn peer_store(&self) -> &dyn PeerStore {
        &self.peer
    }

    fn version_store(&self) -> &dyn VersionStore {
        &self.version
    }

    fn online_weight_store(&self) -> &dyn OnlineWeightStore {
        &self.online_weight
    }

    fn rep_weight_store(&self) -> Arc<dyn RepWeightStore> {
        self.rep_weight.clone()
    }

    #[cfg(feature = "ledger_snapshots")]
    fn forks_store(&self) -> &dyn ForksStore {
        &self.forks
    }

    fn begin_read(&self) -> ReadTransaction {
        self.begin_read()
    }

    fn begin_write(&self) -> WriteTransaction {
        self.begin_write()
    }

    fn refresh_write_txn(&self, txn: WriteTransaction) -> WriteTransaction {
        self.env.refresh(txn)
    }

    fn sync(&self) -> Result<()> {
        self.env.sync().map_err(|e| anyhow!(e))
    }

    fn cache(&self) -> &LedgerCache {
        &self.cache
    }

    fn memory_stats(&self) -> Result<MemoryStats> {
        self.memory_stats()
    }

    fn for_each_account_par(
        &self,
        thread_count: usize,
        action: &(dyn Fn(&mut dyn Iterator<Item = (Account, AccountInfo)>) + Send + Sync),
    ) {
        self.account
            .for_each_par(&self.env, thread_count, |iter| action(iter));
    }

    fn for_each_confirmation_height_par(
        &self,
        thread_count: usize,
        action: &(dyn Fn(&mut dyn Iterator<Item = (Account, ConfirmationHeightInfo)>) + Send + Sync),
    ) {
        self.confirmation_height
            .for_each_par(&self.env, thread_count, |iter| action(iter));
    }
}

impl BlockStore for LmdbBlockStore {
    fn put(&self, txn: &mut WriteTransaction, block: &SavedBlock) {
        self.put(txn, block);
    }

    fn get(&self, txn: &dyn Transaction, hash: &BlockHash) -> Option<SavedBlock> {
        self.get(txn, hash)
    }

    fn del(&self, txn: &mut WriteTransaction, hash: &BlockHash) {
        self.del(txn, hash);
    }

    fn exists(&self, txn: &dyn Transaction, hash: &BlockHash) -> bool {
        self.exists(txn, hash)
    }

    fn iter<'a>(&'a self, txn: &'a dyn Transaction) -> StoreIterator<'a, SavedBlock> {
        Box::new(self.iter(txn))
    }

    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn Transaction,
        range: RangeBounds<BlockHash>,
    ) -> StoreIterator<'a, SavedBlock> {
        Box::new(self.iter_range(txn, range))
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<SavedBlock>> {
        self.track_puts()
    }
}

impl AccountStore for LmdbAccountStore {
    fn put(&self, txn: &mut WriteTransaction, account: &Account, info: &AccountInfo) {
        self.put(txn, account, info);
    }

    fn get(&self, txn: &dyn Transaction, account: &Account) -> Option<AccountInfo> {
        self.get(txn, account)
    }

    fn del(&self, txn: &mut WriteTransaction, account: &Account) {
        self.del(txn, account);
    }

    fn iter<'a>(&'a self, txn: &'a dyn Transaction) -> StoreIterator<'a, (Account, AccountInfo)> {
        Box::new(self.iter(txn))
    }

    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn Transaction,
        range: RangeBounds<Account>,
    ) -> StoreIterator<'a, (Account, AccountInfo)> {
        Box::new(self.iter_range(txn, range))
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(Account, AccountInfo)>> {
        self.track_puts()
    }
}

impl PendingStore for LmdbPendingStore {
    fn put(&self, txn: &mut WriteTransaction, key: &PendingKey, pending: &PendingInfo) {
        self.put(txn, key, pending);
    }

    fn del(&self, txn: &mut WriteTransaction, key: &PendingKey) {
        self.del(txn, key);
    }

    fn get(&self, txn: &dyn Transaction, key: &PendingKey) -> Option<PendingInfo> {
        self.get(txn, key)
    }

    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn Transaction,
        range: RangeBounds<PendingKey>,
    ) -> StoreIterator<'a, (PendingKey, PendingInfo)> {
        Box::new(self.iter_range(txn, range))
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(PendingKey, PendingInfo)>> {
        self.track_puts()
    }

    fn track_deletions(&self) -> Arc<OutputTrackerMt<PendingKey>> {
        self.track_deletions()
    }
}

impl ConfirmationHeightStore for LmdbConfirmationHeightStore {
    fn put(&self, txn: &mut WriteTransaction, account: &Account, info: &ConfirmationHeightInfo) {
        self.put(txn, account, info);
    }

    fn get(&self, txn: &dyn Transaction, account: &Account) -> Option<ConfirmationHeightInfo> {
        self.get(txn, account)
    }

    fn exists(&self, txn: &dyn Transaction, account: &Account) -> bool {
        self.exists(txn, account)
    }

    fn iter<'a>(
        &'a self,
        txn: &'a dyn Transaction,
    ) -> StoreIterator<'a, (Account, ConfirmationHeightInfo)> {
        Box::new(LmdbConfirmationHeightStore::iter(self, txn))
    }
}

impl RepWeightStore for LmdbRepWeightStore {
    fn get(&self, txn: &dyn Transaction, rep: &PublicKey) -> Option<Amount> {
        self.get(txn, rep)
    }

    fn put(&self, txn: &mut WriteTransaction, representative: PublicKey, weight: Amount) {
        self.put(txn, representative, weight);
    }

    fn del(&self, txn: &mut WriteTransaction, representative: &PublicKey) {
        self.del(txn, representative);
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(PublicKey, Amount)>> {
        self.track_puts()
    }

    fn track_deletions(&self) -> Arc<OutputTrackerMt<PublicKey>> {
        self.track_deletions()
    }
}

impl SuccessorStore for LmdbSuccessorStore {
    fn put(&self, txn: &mut WriteTransaction, block: &BlockHash, successor: &BlockHash) {
        self.put(txn, block, successor);
    }

    fn del(&self, txn: &mut WriteTransaction, block: &BlockHash) {
        self.del(txn, block);
    }

    fn get(&self, txn: &dyn Transaction, block: &BlockHash) -> Option<BlockHash> {
        self.get(txn, block)
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(BlockHash, BlockHash)>> {
        self.track_puts()
    }
}

impl FinalVoteStore for LmdbFinalVoteStore {
    fn put(&self, txn: &mut WriteTransaction, root: &QualifiedRoot, hash: &BlockHash) -> bool {
        self.put(txn, root, hash)
    }

    fn get(&self, txn: &dyn Transaction, root: &QualifiedRoot) -> Option<BlockHash> {
        self.get(txn, root)
    }
}

impl PeerStore for LmdbPeerStore {
    fn put(&self, txn: &mut WriteTransaction, endpoint: SocketAddrV6, time: SystemTime) {
        self.put(txn, endpoint, time);
    }

    fn del(&self, txn: &mut WriteTransaction, endpoint: SocketAddrV6) {
        self.del(txn, endpoint);
    }

    fn exists(&self, txn: &dyn Transaction, endpoint: SocketAddrV6) -> bool {
        self.exists(txn, endpoint)
    }

    fn iter<'a>(
        &'a self,
        txn: &'a dyn Transaction,
    ) -> StoreIterator<'a, (SocketAddrV6, SystemTime)> {
        Box::new(self.iter(txn))
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(SocketAddrV6, SystemTime)>> {
        self.track_puts()
    }

    fn track_deletions(&self) -> Arc<OutputTrackerMt<SocketAddrV6>> {
        self.track_deletions()
    }
}

impl OnlineWeightStore for LmdbOnlineWeightStore {
    fn put(&self, txn: &mut WriteTransaction, time: u64, amount: &Amount) {
        self.put(txn, time, amount);
    }

    fn del(&self, txn: &mut WriteTransaction, time: u64) {
        self.del(txn, time);
    }

    fn iter<'a>(&'a self, txn: &'a dyn Transaction) -> StoreIterator<'a, (u64, Amount)> {
        Box::new(LmdbOnlineWeightStore::iter(self, txn))
    }

    fn iter_rev<'a>(&'a self, txn: &'a dyn Transaction) -> StoreIterator<'a, (u64, Amount)> {
        Box::new(LmdbOnlineWeightStore::iter_rev(self, txn))
    }
}

impl VersionStore for LmdbVersionStore {
    fn get(&self, txn: &dyn Transaction) -> Option<i32> {
        self.get(txn)
    }
}

#[cfg(feature = "ledger_snapshots")]
impl ForksStore for LmdbForksStore {
    fn put(&self, txn: &mut WriteTransaction, root: &QualifiedRoot, snapshot: SnapshotNumber) {
        self.put(txn, root, snapshot);
    }

    fn del(&self, txn: &mut WriteTransaction, root: &QualifiedRoot) {
        self.del(txn, root);
    }

    fn get(&self, txn: &dyn Transaction, root: &QualifiedRoot) -> Option<SnapshotNumber> {
        self.get(txn, root)
    }

    fn iter<'a>(
        &'a self,
        txn: &'a dyn Transaction,
    ) -> StoreIterator<'a, (QualifiedRoot, SnapshotNumber)> {
        Box::new(LmdbForksStore::iter(self, txn))
    }
}
