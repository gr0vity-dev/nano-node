use std::{
    fmt,
    net::SocketAddrV6,
    ops::{Bound, RangeBounds as StdRangeBounds},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

use crate::{LedgerReadTxn, LedgerWriteTxn};
use anyhow::Result;
use rsnano_output_tracker::OutputTrackerMt;
#[cfg(feature = "ledger_snapshots")]
use rsnano_types::SnapshotNumber;
use rsnano_types::{
    Account, AccountInfo, Amount, BlockHash, ConfirmationHeightInfo, PendingInfo, PendingKey,
    PublicKey, QualifiedRoot, SavedBlock,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriterType {
    Testing,
    BlockProcessor,
    ConfirmationHeight,
    RepWeights,
    RepWeightUpdater,
    VotingFinalizer,
    Bootstrap,
    Generic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteStrategy {
    Pessimistic,
    Optimistic,
}

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

    fn begin_read(&self) -> Box<dyn LedgerReadTxn>;

    fn begin_write(&self) -> Box<dyn LedgerWriteTxn> {
        self.begin_write_with_writer(WriterType::Generic, WriteStrategy::Optimistic)
    }

    fn begin_write_with_writer(
        &self,
        writer: WriterType,
        strategy: WriteStrategy,
    ) -> Box<dyn LedgerWriteTxn>;

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
        action: &(
             dyn Fn(&mut dyn Iterator<Item = (Account, ConfirmationHeightInfo)>) + Send + Sync
         ),
    );

    fn vendor(&self) -> StoreVendor;
}

pub trait BlockStore: Send + Sync {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, block: &SavedBlock);
    fn get(&self, txn: &dyn LedgerReadTxn, hash: &BlockHash) -> Option<SavedBlock>;
    fn del(&self, txn: &mut dyn LedgerWriteTxn, hash: &BlockHash);
    fn exists(&self, txn: &dyn LedgerReadTxn, hash: &BlockHash) -> bool;
    fn iter<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, SavedBlock>;
    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
        range: RangeBounds<BlockHash>,
    ) -> StoreIterator<'a, SavedBlock>;
    fn track_puts(&self) -> Arc<OutputTrackerMt<SavedBlock>>;
}

pub trait AccountStore: Send + Sync {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, account: &Account, info: &AccountInfo);
    fn get(&self, txn: &dyn LedgerReadTxn, account: &Account) -> Option<AccountInfo>;
    fn del(&self, txn: &mut dyn LedgerWriteTxn, account: &Account);
    fn iter<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, (Account, AccountInfo)>;
    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
        range: RangeBounds<Account>,
    ) -> StoreIterator<'a, (Account, AccountInfo)>;
    fn track_puts(&self) -> Arc<OutputTrackerMt<(Account, AccountInfo)>>;
}

pub trait PendingStore: Send + Sync {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey, pending: &PendingInfo);
    fn del(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey);
    fn get(&self, txn: &dyn LedgerReadTxn, key: &PendingKey) -> Option<PendingInfo>;
    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
        range: RangeBounds<PendingKey>,
    ) -> StoreIterator<'a, (PendingKey, PendingInfo)>;
    fn track_puts(&self) -> Arc<OutputTrackerMt<(PendingKey, PendingInfo)>>;
    fn track_deletions(&self) -> Arc<OutputTrackerMt<PendingKey>>;
}

pub trait ConfirmationHeightStore: Send + Sync {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, account: &Account, info: &ConfirmationHeightInfo);
    fn get(&self, txn: &dyn LedgerReadTxn, account: &Account) -> Option<ConfirmationHeightInfo>;
    fn exists(&self, txn: &dyn LedgerReadTxn, account: &Account) -> bool;
    fn iter<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
    ) -> StoreIterator<'a, (Account, ConfirmationHeightInfo)>;
}

pub trait RepWeightStore: Send + Sync {
    fn get(&self, txn: &dyn LedgerReadTxn, rep: &PublicKey) -> Option<Amount>;
    fn put(&self, txn: &mut dyn LedgerWriteTxn, representative: PublicKey, weight: Amount);
    fn del(&self, txn: &mut dyn LedgerWriteTxn, representative: &PublicKey);
    fn track_puts(&self) -> Arc<OutputTrackerMt<(PublicKey, Amount)>>;
    fn track_deletions(&self) -> Arc<OutputTrackerMt<PublicKey>>;
}

pub trait SuccessorStore: Send + Sync {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, block: &BlockHash, successor: &BlockHash);
    fn del(&self, txn: &mut dyn LedgerWriteTxn, block: &BlockHash);
    fn get(&self, txn: &dyn LedgerReadTxn, block: &BlockHash) -> Option<BlockHash>;
    fn track_puts(&self) -> Arc<OutputTrackerMt<(BlockHash, BlockHash)>>;
}

pub trait FinalVoteStore: Send + Sync {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, root: &QualifiedRoot, hash: &BlockHash) -> bool;
    fn get(&self, txn: &dyn LedgerReadTxn, root: &QualifiedRoot) -> Option<BlockHash>;
}

pub trait PeerStore: Send + Sync {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, endpoint: SocketAddrV6, time: SystemTime);
    fn del(&self, txn: &mut dyn LedgerWriteTxn, endpoint: SocketAddrV6);
    fn exists(&self, txn: &dyn LedgerReadTxn, endpoint: SocketAddrV6) -> bool;
    fn iter<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
    ) -> StoreIterator<'a, (SocketAddrV6, SystemTime)>;
    fn track_puts(&self) -> Arc<OutputTrackerMt<(SocketAddrV6, SystemTime)>>;
    fn track_deletions(&self) -> Arc<OutputTrackerMt<SocketAddrV6>>;
}

pub trait OnlineWeightStore: Send + Sync {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, time: u64, amount: &Amount);
    fn del(&self, txn: &mut dyn LedgerWriteTxn, time: u64);
    fn iter<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, (u64, Amount)>;
    fn iter_rev<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, (u64, Amount)>;
}

pub trait VersionStore: Send + Sync {
    fn get(&self, txn: &dyn LedgerReadTxn) -> Option<i32>;
}

#[cfg(feature = "ledger_snapshots")]
pub trait ForksStore: Send + Sync {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, root: &QualifiedRoot, snapshot: SnapshotNumber);
    fn del(&self, txn: &mut dyn LedgerWriteTxn, root: &QualifiedRoot);
    fn get(&self, txn: &dyn LedgerReadTxn, root: &QualifiedRoot) -> Option<SnapshotNumber>;
    fn iter<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
    ) -> StoreIterator<'a, (QualifiedRoot, SnapshotNumber)>;
}

#[derive(Serialize, Deserialize)]
pub struct MemoryStats {
    pub branch_pages: usize,
    pub depth: u32,
    pub entries: usize,
    pub leaf_pages: usize,
    pub overflow_pages: usize,
    pub page_size: u32,
}

pub struct LedgerCache {
    pub confirmed_count: AtomicU64,
    pub block_count: AtomicU64,
    pub account_count: AtomicU64,
}

impl LedgerCache {
    pub fn new() -> Self {
        Self {
            confirmed_count: AtomicU64::new(0),
            block_count: AtomicU64::new(0),
            account_count: AtomicU64::new(0),
        }
    }

    pub fn reset(&self) {
        self.confirmed_count.store(0, Ordering::SeqCst);
        self.block_count.store(0, Ordering::SeqCst);
        self.account_count.store(0, Ordering::SeqCst);
    }
}

pub trait LedgerStoreFactory: Send + Sync {
    fn create_store(
        &self,
        path: PathBuf,
        config: crate::config::LedgerStoreConfig,
        cache: Arc<LedgerCache>,
    ) -> anyhow::Result<Arc<dyn LedgerStore>>;

    fn create_null_store(&self, cache: Arc<LedgerCache>) -> anyhow::Result<Arc<dyn LedgerStore>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreVendor {
    pub name: &'static str,
    pub version: String,
}

impl StoreVendor {
    pub fn new(name: &'static str, version: impl Into<String>) -> Self {
        Self {
            name,
            version: version.into(),
        }
    }
}

impl fmt::Display for StoreVendor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.version.is_empty() {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{} {}", self.name, self.version)
        }
    }
}
