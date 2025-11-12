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
use store_traits::transaction::{LedgerReadTxn, LedgerWriteTxn};

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
    fn put(&self, txn: &mut dyn LedgerWriteTxn, block: &SavedBlock) {
        LmdbBlockStore::put(self, write_txn_shim(txn), block);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, hash: &BlockHash) -> Option<SavedBlock> {
        LmdbBlockStore::get(self, read_txn_shim(txn), hash)
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, hash: &BlockHash) {
        LmdbBlockStore::del(self, write_txn_shim(txn), hash);
    }

    fn exists(&self, txn: &dyn LedgerReadTxn, hash: &BlockHash) -> bool {
        LmdbBlockStore::exists(self, read_txn_shim(txn), hash)
    }

    fn iter<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, SavedBlock> {
        Box::new(LmdbBlockStore::iter(self, read_txn_shim(txn)))
    }

    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
        range: RangeBounds<BlockHash>,
    ) -> StoreIterator<'a, SavedBlock> {
        Box::new(LmdbBlockStore::iter_range(self, read_txn_shim(txn), range))
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<SavedBlock>> {
        self.track_puts()
    }
}

impl AccountStore for LmdbAccountStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, account: &Account, info: &AccountInfo) {
        LmdbAccountStore::put(self, write_txn_shim(txn), account, info);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, account: &Account) -> Option<AccountInfo> {
        LmdbAccountStore::get(self, read_txn_shim(txn), account)
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, account: &Account) {
        LmdbAccountStore::del(self, write_txn_shim(txn), account);
    }

    fn iter<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, (Account, AccountInfo)> {
        Box::new(LmdbAccountStore::iter(self, read_txn_shim(txn)))
    }

    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
        range: RangeBounds<Account>,
    ) -> StoreIterator<'a, (Account, AccountInfo)> {
        Box::new(LmdbAccountStore::iter_range(
            self,
            read_txn_shim(txn),
            range,
        ))
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(Account, AccountInfo)>> {
        self.track_puts()
    }
}

impl PendingStore for LmdbPendingStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey, pending: &PendingInfo) {
        LmdbPendingStore::put(self, write_txn_shim(txn), key, pending);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey) {
        LmdbPendingStore::del(self, write_txn_shim(txn), key);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, key: &PendingKey) -> Option<PendingInfo> {
        LmdbPendingStore::get(self, read_txn_shim(txn), key)
    }

    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
        range: RangeBounds<PendingKey>,
    ) -> StoreIterator<'a, (PendingKey, PendingInfo)> {
        Box::new(LmdbPendingStore::iter_range(
            self,
            read_txn_shim(txn),
            range,
        ))
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(PendingKey, PendingInfo)>> {
        self.track_puts()
    }

    fn track_deletions(&self) -> Arc<OutputTrackerMt<PendingKey>> {
        self.track_deletions()
    }
}

impl ConfirmationHeightStore for LmdbConfirmationHeightStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, account: &Account, info: &ConfirmationHeightInfo) {
        LmdbConfirmationHeightStore::put(self, write_txn_shim(txn), account, info);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, account: &Account) -> Option<ConfirmationHeightInfo> {
        LmdbConfirmationHeightStore::get(self, read_txn_shim(txn), account)
    }

    fn exists(&self, txn: &dyn LedgerReadTxn, account: &Account) -> bool {
        LmdbConfirmationHeightStore::exists(self, read_txn_shim(txn), account)
    }

    fn iter<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
    ) -> StoreIterator<'a, (Account, ConfirmationHeightInfo)> {
        Box::new(LmdbConfirmationHeightStore::iter(self, read_txn_shim(txn)))
    }
}

impl RepWeightStore for LmdbRepWeightStore {
    fn get(&self, txn: &dyn LedgerReadTxn, rep: &PublicKey) -> Option<Amount> {
        LmdbRepWeightStore::get(self, read_txn_shim(txn), rep)
    }

    fn put(&self, txn: &mut dyn LedgerWriteTxn, representative: PublicKey, weight: Amount) {
        LmdbRepWeightStore::put(self, write_txn_shim(txn), representative, weight);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, representative: &PublicKey) {
        LmdbRepWeightStore::del(self, write_txn_shim(txn), representative);
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(PublicKey, Amount)>> {
        self.track_puts()
    }

    fn track_deletions(&self) -> Arc<OutputTrackerMt<PublicKey>> {
        self.track_deletions()
    }
}

impl SuccessorStore for LmdbSuccessorStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, block: &BlockHash, successor: &BlockHash) {
        LmdbSuccessorStore::put(self, write_txn_shim(txn), block, successor);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, block: &BlockHash) {
        LmdbSuccessorStore::del(self, write_txn_shim(txn), block);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, block: &BlockHash) -> Option<BlockHash> {
        LmdbSuccessorStore::get(self, read_txn_shim(txn), block)
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(BlockHash, BlockHash)>> {
        self.track_puts()
    }
}

impl FinalVoteStore for LmdbFinalVoteStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, root: &QualifiedRoot, hash: &BlockHash) -> bool {
        LmdbFinalVoteStore::put(self, write_txn_shim(txn), root, hash)
    }

    fn get(&self, txn: &dyn LedgerReadTxn, root: &QualifiedRoot) -> Option<BlockHash> {
        LmdbFinalVoteStore::get(self, read_txn_shim(txn), root)
    }
}

impl PeerStore for LmdbPeerStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, endpoint: SocketAddrV6, time: SystemTime) {
        LmdbPeerStore::put(self, write_txn_shim(txn), endpoint, time);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, endpoint: SocketAddrV6) {
        LmdbPeerStore::del(self, write_txn_shim(txn), endpoint);
    }

    fn exists(&self, txn: &dyn LedgerReadTxn, endpoint: SocketAddrV6) -> bool {
        LmdbPeerStore::exists(self, read_txn_shim(txn), endpoint)
    }

    fn iter<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
    ) -> StoreIterator<'a, (SocketAddrV6, SystemTime)> {
        Box::new(LmdbPeerStore::iter(self, read_txn_shim(txn)))
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(SocketAddrV6, SystemTime)>> {
        self.track_puts()
    }

    fn track_deletions(&self) -> Arc<OutputTrackerMt<SocketAddrV6>> {
        self.track_deletions()
    }
}

impl OnlineWeightStore for LmdbOnlineWeightStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, time: u64, amount: &Amount) {
        LmdbOnlineWeightStore::put(self, write_txn_shim(txn), time, amount);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, time: u64) {
        LmdbOnlineWeightStore::del(self, write_txn_shim(txn), time);
    }

    fn iter<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, (u64, Amount)> {
        Box::new(LmdbOnlineWeightStore::iter(self, read_txn_shim(txn)))
    }

    fn iter_rev<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, (u64, Amount)> {
        Box::new(LmdbOnlineWeightStore::iter_rev(self, read_txn_shim(txn)))
    }
}

impl VersionStore for LmdbVersionStore {
    fn get(&self, txn: &dyn LedgerReadTxn) -> Option<i32> {
        LmdbVersionStore::get(self, read_txn_shim(txn))
    }
}

#[cfg(feature = "ledger_snapshots")]
impl ForksStore for LmdbForksStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, root: &QualifiedRoot, snapshot: SnapshotNumber) {
        LmdbForksStore::put(self, write_txn_shim(txn), root, snapshot);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, root: &QualifiedRoot) {
        LmdbForksStore::del(self, write_txn_shim(txn), root);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, root: &QualifiedRoot) -> Option<SnapshotNumber> {
        LmdbForksStore::get(self, read_txn_shim(txn), root)
    }

    fn iter<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
    ) -> StoreIterator<'a, (QualifiedRoot, SnapshotNumber)> {
        Box::new(LmdbForksStore::iter(self, read_txn_shim(txn)))
    }
}
fn read_txn_shim(txn: &dyn LedgerReadTxn) -> &dyn Transaction {
    txn.as_lmdb_txn_shim()
}

fn write_txn_shim(txn: &mut dyn LedgerWriteTxn) -> &mut WriteTransaction {
    txn.as_lmdb_write_txn_shim()
}
