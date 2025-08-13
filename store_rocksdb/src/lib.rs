use anyhow::Result;
use rocksdb::{Options, DB};
use std::sync::Arc;
use store_api::{ReadTxnLike, StoreProvider, TransactionLike, VersionStore, WriteTxnLike, PrunedStore as PrunedStoreApi, BlockStore as BlockStoreApi, AccountStore as AccountStoreApi, ConfirmationHeightStore as ConfirmationHeightStoreApi, PendingStore as PendingStoreApi, SuccessorStore as SuccessorStoreApi, FinalVoteStore as FinalVoteStoreApi};

pub struct RocksProvider {
    db: Arc<DB>,
    version: RocksVersionStore,
    pruned: RocksPrunedStore,
    block: RocksBlockStore,
    account: RocksAccountStore,
    confirmation_height: RocksConfirmationHeightStore,
    pending: RocksPendingStore,
    successor: RocksSuccessorStore,
    final_vote: RocksFinalVoteStore,
}

impl RocksProvider {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        let db = Arc::new(DB::open(&opts, path)?);
        Ok(Self { version: RocksVersionStore { db: db.clone() }, pruned: RocksPrunedStore { db: db.clone() }, block: RocksBlockStore { db: db.clone() }, account: RocksAccountStore { db: db.clone() }, confirmation_height: RocksConfirmationHeightStore { db: db.clone() }, pending: RocksPendingStore { db: db.clone() }, successor: RocksSuccessorStore { db: db.clone() }, final_vote: RocksFinalVoteStore { db: db.clone() }, db })
    }
}

use rocksdb::WriteBatch;

pub struct RocksReadTxn;
pub struct RocksWriteTxn {
    batch: WriteBatch,
}

impl TransactionLike for RocksReadTxn {
    fn is_refresh_needed(&self) -> bool { false }
}
impl ReadTxnLike for RocksReadTxn {}

impl TransactionLike for RocksWriteTxn {
    fn is_refresh_needed(&self) -> bool { false }
}
impl WriteTxnLike for RocksWriteTxn {}

impl StoreProvider for RocksProvider {
    type ReadTxn = RocksReadTxn;
    type WriteTxn = RocksWriteTxn;

    fn begin_read(&self) -> Self::ReadTxn { RocksReadTxn }
    fn begin_write(&self) -> Self::WriteTxn { RocksWriteTxn { batch: WriteBatch::default() } }
    fn refresh(&self, write: Self::WriteTxn) -> Self::WriteTxn { write }
    fn commit(&self, write: Self::WriteTxn) {
        let _ = self.db.write(write.batch);
        let _ = self.db.flush();
    }

    type Version = RocksVersionStore;
    fn version(&self) -> &Self::Version { &self.version }
    type Pruned = RocksPrunedStore;
    fn pruned(&self) -> &Self::Pruned { &self.pruned }
    type Block = RocksBlockStore;
    fn block(&self) -> &Self::Block { &self.block }
    type Account = RocksAccountStore;
    fn account(&self) -> &Self::Account { &self.account }
    type ConfirmationHeight = RocksConfirmationHeightStore;
    fn confirmation_height(&self) -> &Self::ConfirmationHeight { &self.confirmation_height }
    type Pending = RocksPendingStore;
    fn pending(&self) -> &Self::Pending { &self.pending }
    type Successor = RocksSuccessorStore;
    fn successor(&self) -> &Self::Successor { &self.successor }
    type FinalVote = RocksFinalVoteStore;
    fn final_vote(&self) -> &Self::FinalVote { &self.final_vote }
}

pub struct RocksVersionStore {
    db: Arc<DB>,
}

const META_VERSION_KEY: &[u8] = b"meta:version";

impl VersionStore<RocksReadTxn, RocksWriteTxn> for RocksVersionStore {
    fn get(&self, _read: &RocksReadTxn) -> Option<i32> {
        self.db.get(META_VERSION_KEY).ok().flatten().map(|v| {
            let mut arr = [0u8; 4];
            arr.copy_from_slice(&v);
            i32::from_be_bytes(arr)
        })
    }

    fn set(&self, write: &mut RocksWriteTxn, version: i32) {
        let _ = write.batch.put(META_VERSION_KEY, version.to_be_bytes());
    }
}

pub struct RocksPrunedStore { db: Arc<DB> }

const PRUNED_PREFIX: &[u8] = b"pruned:";

impl PrunedStoreApi<RocksReadTxn, RocksWriteTxn> for RocksPrunedStore {
    fn count(&self, _read: &RocksReadTxn) -> u64 {
        // Simplified: Rocks doesn't expose count per prefix cheaply; return 0 in MVP (not used in swap yet)
        0
    }
    fn exists(&self, _read: &RocksReadTxn, hash: &rsnano_core::BlockHash) -> bool {
        let mut key = PRUNED_PREFIX.to_vec();
        key.extend_from_slice(hash.as_bytes());
        self.db.get(key).ok().flatten().is_some()
    }
    fn put(&self, write: &mut RocksWriteTxn, hash: &rsnano_core::BlockHash) {
        let mut key = PRUNED_PREFIX.to_vec();
        key.extend_from_slice(hash.as_bytes());
        let _ = write.batch.put(key, &[]);
    }
    fn del(&self, write: &mut RocksWriteTxn, hash: &rsnano_core::BlockHash) {
        let mut key = PRUNED_PREFIX.to_vec();
        key.extend_from_slice(hash.as_bytes());
        let _ = write.batch.delete(key);
    }
}

pub struct RocksBlockStore { db: Arc<DB> }

impl BlockStoreApi<RocksReadTxn, RocksWriteTxn> for RocksBlockStore {
    fn exists(&self, _read: &RocksReadTxn, hash: &rsnano_core::BlockHash) -> bool {
        let mut key = b"block:".to_vec();
        key.extend_from_slice(hash.as_bytes());
        self.db.get(key).ok().flatten().is_some()
    }
    fn get(&self, _read: &RocksReadTxn, _hash: &rsnano_core::BlockHash) -> Option<rsnano_core::SavedBlock> {
        None
    }
    fn del(&self, write: &mut RocksWriteTxn, hash: &rsnano_core::BlockHash) {
        let mut key = b"block:".to_vec();
        key.extend_from_slice(hash.as_bytes());
        let _ = write.batch.delete(key);
    }
}

pub struct RocksAccountStore { db: Arc<DB> }
pub struct RocksConfirmationHeightStore { db: Arc<DB> }
pub struct RocksPendingStore { db: Arc<DB> }
pub struct RocksSuccessorStore { db: Arc<DB> }
pub struct RocksFinalVoteStore { db: Arc<DB> }

impl AccountStoreApi<RocksReadTxn, RocksWriteTxn> for RocksAccountStore {
    fn count(&self, _read: &RocksReadTxn) -> u64 { 0 }
    fn get(&self, _read: &RocksReadTxn, _account: &rsnano_core::Account) -> Option<rsnano_core::AccountInfo> { None }
    fn iter<'a>(&'a self, _read: &'a RocksReadTxn) -> Box<dyn Iterator<Item = (rsnano_core::Account, rsnano_core::AccountInfo)> + 'a> {
        Box::new(std::iter::empty())
    }
}

impl ConfirmationHeightStoreApi<RocksReadTxn, RocksWriteTxn> for RocksConfirmationHeightStore {
    fn count(&self, _read: &RocksReadTxn) -> u64 { 0 }
    fn get(&self, _read: &RocksReadTxn, _account: &rsnano_core::Account) -> Option<rsnano_core::ConfirmationHeightInfo> { None }
    fn iter<'a>(&'a self, _read: &'a RocksReadTxn) -> Box<dyn Iterator<Item = (rsnano_core::Account, rsnano_core::ConfirmationHeightInfo)> + 'a> {
        Box::new(std::iter::empty())
    }
}

impl PendingStoreApi<RocksReadTxn, RocksWriteTxn> for RocksPendingStore {
    fn get(&self, _read: &RocksReadTxn, _key: &rsnano_core::PendingKey) -> Option<rsnano_core::PendingInfo> { None }
    fn exists(&self, _read: &RocksReadTxn, _key: &rsnano_core::PendingKey) -> bool { false }
}

impl SuccessorStoreApi<RocksReadTxn, RocksWriteTxn> for RocksSuccessorStore {
    fn get(&self, _read: &RocksReadTxn, _block: &rsnano_core::BlockHash) -> Option<rsnano_core::BlockHash> { None }
}

impl FinalVoteStoreApi<RocksReadTxn, RocksWriteTxn> for RocksFinalVoteStore {
    fn get(&self, _read: &RocksReadTxn, _root: &rsnano_core::QualifiedRoot) -> Option<rsnano_core::BlockHash> { None }
    fn put(&self, _write: &mut RocksWriteTxn, _root: &rsnano_core::QualifiedRoot, _hash: &rsnano_core::BlockHash) -> bool { false }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn version_roundtrip() -> Result<()> {
        let dir = tempdir()?;
        let provider = RocksProvider::open(dir.path())?;

        let mut w = provider.begin_write();
        provider.version().set(&mut w, 777);

        // Not visible until commit
        let r = provider.begin_read();
        assert_eq!(provider.version().get(&r), None);

        provider.commit(w);

        let r = provider.begin_read();
        assert_eq!(provider.version().get(&r), Some(777));
        Ok(())
    }
}
