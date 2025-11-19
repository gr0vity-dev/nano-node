use std::{
    net::SocketAddrV6,
    sync::{Arc, OnceLock},
    time::SystemTime,
};

use anyhow::anyhow;
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
    StoreVendor, SuccessorStore, VersionStore, WriteStrategy, WriterType,
};
use store_traits::transaction::{LedgerReadTxn, LedgerWriteTxn};

use crate::transaction::{LmdbLedgerReadTxn, LmdbLedgerWriteTxn};

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

    fn begin_read(&self) -> Box<dyn LedgerReadTxn> {
        Box::new(LmdbLedgerReadTxn::new(self.env.begin_read()))
    }

    fn begin_write_with_writer(
        &self,
        _writer: WriterType,
        _strategy: WriteStrategy,
    ) -> Box<dyn LedgerWriteTxn> {
        Box::new(LmdbLedgerWriteTxn::new(self.env.begin_write()))
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

    fn vendor(&self) -> StoreVendor {
        lmdb_vendor()
    }
}

fn lmdb_vendor() -> StoreVendor {
    static VENDOR: OnceLock<StoreVendor> = OnceLock::new();
    VENDOR
        .get_or_init(|| {
            let mut major = 0;
            let mut minor = 0;
            let mut patch = 0;
            unsafe {
                rsnano_nullable_lmdb::sys::mdb_version(&mut major, &mut minor, &mut patch);
            }
            StoreVendor::new("lmdb", format!("{major}.{minor}.{patch}"))
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::lmdb_vendor;

    #[test]
    fn vendor_matches_lmdb_version() {
        let vendor = lmdb_vendor();
        assert_eq!(vendor.name, "lmdb");

        let mut major = 0;
        let mut minor = 0;
        let mut patch = 0;
        unsafe {
            rsnano_nullable_lmdb::sys::mdb_version(&mut major, &mut minor, &mut patch);
        }

        assert_eq!(vendor.version, format!("{major}.{minor}.{patch}"));
    }
}

impl BlockStore for LmdbBlockStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, block: &SavedBlock) {
        LmdbBlockStore::put(self, txn, block);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, hash: &BlockHash) -> Option<SavedBlock> {
        LmdbBlockStore::get(self, txn, hash)
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, hash: &BlockHash) {
        LmdbBlockStore::del(self, txn, hash);
    }

    fn exists(&self, txn: &dyn LedgerReadTxn, hash: &BlockHash) -> bool {
        LmdbBlockStore::exists(self, txn, hash)
    }

    fn iter<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, SavedBlock> {
        Box::new(LmdbBlockStore::iter(self, txn))
    }

    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
        range: RangeBounds<BlockHash>,
    ) -> StoreIterator<'a, SavedBlock> {
        Box::new(LmdbBlockStore::iter_range(self, txn, range))
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<SavedBlock>> {
        self.track_puts()
    }
}

impl AccountStore for LmdbAccountStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, account: &Account, info: &AccountInfo) {
        LmdbAccountStore::put(self, txn, account, info);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, account: &Account) -> Option<AccountInfo> {
        LmdbAccountStore::get(self, txn, account)
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, account: &Account) {
        LmdbAccountStore::del(self, txn, account);
    }

    fn iter<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, (Account, AccountInfo)> {
        Box::new(LmdbAccountStore::iter(self, txn))
    }

    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
        range: RangeBounds<Account>,
    ) -> StoreIterator<'a, (Account, AccountInfo)> {
        Box::new(LmdbAccountStore::iter_range(self, txn, range))
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(Account, AccountInfo)>> {
        self.track_puts()
    }
}

impl PendingStore for LmdbPendingStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey, pending: &PendingInfo) {
        LmdbPendingStore::put(self, txn, key, pending);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey) {
        LmdbPendingStore::del(self, txn, key);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, key: &PendingKey) -> Option<PendingInfo> {
        LmdbPendingStore::get(self, txn, key)
    }

    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
        range: RangeBounds<PendingKey>,
    ) -> StoreIterator<'a, (PendingKey, PendingInfo)> {
        Box::new(LmdbPendingStore::iter_range(self, txn, range))
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
        LmdbConfirmationHeightStore::put(self, txn, account, info);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, account: &Account) -> Option<ConfirmationHeightInfo> {
        LmdbConfirmationHeightStore::get(self, txn, account)
    }

    fn exists(&self, txn: &dyn LedgerReadTxn, account: &Account) -> bool {
        LmdbConfirmationHeightStore::exists(self, txn, account)
    }

    fn iter<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
    ) -> StoreIterator<'a, (Account, ConfirmationHeightInfo)> {
        Box::new(LmdbConfirmationHeightStore::iter(self, txn))
    }
}

impl RepWeightStore for LmdbRepWeightStore {
    fn get(&self, txn: &dyn LedgerReadTxn, rep: &PublicKey) -> Option<Amount> {
        LmdbRepWeightStore::get(self, txn, rep)
    }

    fn put(&self, txn: &mut dyn LedgerWriteTxn, representative: PublicKey, weight: Amount) {
        LmdbRepWeightStore::put(self, txn, representative, weight);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, representative: &PublicKey) {
        LmdbRepWeightStore::del(self, txn, representative);
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
        LmdbSuccessorStore::put(self, txn, block, successor);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, block: &BlockHash) {
        LmdbSuccessorStore::del(self, txn, block);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, block: &BlockHash) -> Option<BlockHash> {
        LmdbSuccessorStore::get(self, txn, block)
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(BlockHash, BlockHash)>> {
        self.track_puts()
    }
}

impl FinalVoteStore for LmdbFinalVoteStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, root: &QualifiedRoot, hash: &BlockHash) -> bool {
        LmdbFinalVoteStore::put(self, txn, root, hash)
    }

    fn get(&self, txn: &dyn LedgerReadTxn, root: &QualifiedRoot) -> Option<BlockHash> {
        LmdbFinalVoteStore::get(self, txn, root)
    }
}

impl PeerStore for LmdbPeerStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, endpoint: SocketAddrV6, time: SystemTime) {
        LmdbPeerStore::put(self, txn, endpoint, time);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, endpoint: SocketAddrV6) {
        LmdbPeerStore::del(self, txn, endpoint);
    }

    fn exists(&self, txn: &dyn LedgerReadTxn, endpoint: SocketAddrV6) -> bool {
        LmdbPeerStore::exists(self, txn, endpoint)
    }

    fn iter<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
    ) -> StoreIterator<'a, (SocketAddrV6, SystemTime)> {
        Box::new(LmdbPeerStore::iter(self, txn))
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
        LmdbOnlineWeightStore::put(self, txn, time, amount);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, time: u64) {
        LmdbOnlineWeightStore::del(self, txn, time);
    }

    fn iter<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, (u64, Amount)> {
        Box::new(LmdbOnlineWeightStore::iter(self, txn))
    }

    fn iter_rev<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, (u64, Amount)> {
        Box::new(LmdbOnlineWeightStore::iter_rev(self, txn))
    }
}

impl VersionStore for LmdbVersionStore {
    fn get(&self, txn: &dyn LedgerReadTxn) -> Option<i32> {
        LmdbVersionStore::get(self, txn)
    }
}

#[cfg(feature = "ledger_snapshots")]
impl ForksStore for LmdbForksStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, root: &QualifiedRoot, snapshot: SnapshotNumber) {
        LmdbForksStore::put(self, txn, root, snapshot);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, root: &QualifiedRoot) {
        LmdbForksStore::del(self, txn, root);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, root: &QualifiedRoot) -> Option<SnapshotNumber> {
        LmdbForksStore::get(self, txn, root)
    }

    fn iter<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
    ) -> StoreIterator<'a, (QualifiedRoot, SnapshotNumber)> {
        Box::new(LmdbForksStore::iter(self, txn))
    }
}
