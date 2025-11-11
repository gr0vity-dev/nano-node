use std::{net::SocketAddrV6, sync::Arc, time::SystemTime};

use anyhow::anyhow;
use rsnano_nullable_lmdb::{ReadTransaction, Transaction, WriteTransaction};
use rsnano_output_tracker::OutputTrackerMt;
use rsnano_types::{
    Account, AccountInfo, Amount, BlockHash, ConfirmationHeightInfo, PendingInfo, PendingKey,
    PublicKey, QualifiedRoot, SavedBlock,
};
#[cfg(feature = "ledger_snapshots")]
use store_traits::ledger::SnapshotNumber;
use store_traits::ledger::{
    AccountStore, BlockStore, ConfirmationHeightStore, FinalVoteStore, LedgerStore,
    OnlineWeightStore, PeerStore, PendingStore, RangeBounds, RepWeightStore, StoreIterator,
    SuccessorStore, VersionStore,
};

#[cfg(feature = "ledger_snapshots")]
use crate::forks_store::LmdbForksStore;
use crate::{
    LmdbAccountStore, LmdbBlockStore, LmdbConfirmationHeightStore, LmdbFinalVoteStore,
    LmdbOnlineWeightStore, LmdbPeerStore, LmdbPendingStore, LmdbRepWeightStore, LmdbStore,
    LmdbSuccessorStore, LmdbVersionStore,
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
        self.env.begin_read()
    }

    fn begin_write(&self) -> WriteTransaction {
        self.env.begin_write()
    }

    fn refresh_write_txn(&self, txn: WriteTransaction) -> WriteTransaction {
        self.env.refresh(txn)
    }

    fn sync(&self) -> anyhow::Result<()> {
        self.env.sync().map_err(|e| anyhow!(e))
    }

    fn cache(&self) -> &store_traits::ledger::LedgerCache {
        &self.cache
    }

    fn memory_stats(&self) -> anyhow::Result<store_traits::ledger::MemoryStats> {
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
        action: &(
             dyn Fn(&mut dyn Iterator<Item = (Account, ConfirmationHeightInfo)>) + Send + Sync
         ),
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
        Box::new(self.iter(txn))
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
        Box::new(self.iter(txn))
    }

    fn iter_rev<'a>(&'a self, txn: &'a dyn Transaction) -> StoreIterator<'a, (u64, Amount)> {
        Box::new(self.iter_rev(txn))
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
        Box::new(self.iter(txn))
    }
}
