use rsnano_core::{Account, AccountInfo, ConfirmationHeightInfo};

pub trait AccountStore<R: ReadTxnLike, W: WriteTxnLike> {
    fn count(&self, read: &R) -> u64;
    fn get(&self, read: &R, account: &Account) -> Option<AccountInfo>;
    fn iter<'a>(&'a self, read: &'a R) -> Box<dyn Iterator<Item = (Account, AccountInfo)> + 'a>;
}

pub trait ConfirmationHeightStore<R: ReadTxnLike, W: WriteTxnLike> {
    fn count(&self, read: &R) -> u64;
    fn get(&self, read: &R, account: &Account) -> Option<ConfirmationHeightInfo>;
    fn iter<'a>(&'a self, read: &'a R) -> Box<dyn Iterator<Item = (Account, ConfirmationHeightInfo)> + 'a>;
}

pub trait PendingStore<R: ReadTxnLike, W: WriteTxnLike> {
    fn get(&self, read: &R, key: &rsnano_core::PendingKey) -> Option<rsnano_core::PendingInfo>;
    fn exists(&self, read: &R, key: &rsnano_core::PendingKey) -> bool;
}
pub trait TransactionLike {
    fn is_refresh_needed(&self) -> bool;
}

pub trait ReadTxnLike: TransactionLike {}

pub trait WriteTxnLike: TransactionLike {}

pub trait VersionStore<R: ReadTxnLike, W: WriteTxnLike> {
    fn get(&self, read: &R) -> Option<i32>;
    fn set(&self, write: &mut W, version: i32);
}

pub trait StoreProvider {
    type ReadTxn: ReadTxnLike;
    type WriteTxn: WriteTxnLike;
    type Version: VersionStore<Self::ReadTxn, Self::WriteTxn>;
    type Pruned: PrunedStore<Self::ReadTxn, Self::WriteTxn>;
    type Block: BlockStore<Self::ReadTxn, Self::WriteTxn>;
    type Account: AccountStore<Self::ReadTxn, Self::WriteTxn>;
    type ConfirmationHeight: ConfirmationHeightStore<Self::ReadTxn, Self::WriteTxn>;
    type Pending: PendingStore<Self::ReadTxn, Self::WriteTxn>;

    fn begin_read(&self) -> Self::ReadTxn;
    fn begin_write(&self) -> Self::WriteTxn;
    fn refresh(&self, write: Self::WriteTxn) -> Self::WriteTxn;
    fn commit(&self, write: Self::WriteTxn);

    fn version(&self) -> &Self::Version;
    fn pruned(&self) -> &Self::Pruned;
    fn block(&self) -> &Self::Block;
    fn account(&self) -> &Self::Account;
    fn confirmation_height(&self) -> &Self::ConfirmationHeight;
    fn pending(&self) -> &Self::Pending;
}

// Minimal RepWeightStore used by RepWeightsUpdater init path
pub trait RepWeightStore<R: ReadTxnLike, W: WriteTxnLike> {
    fn get(&self, read: &R, rep: &rsnano_core::PublicKey) -> Option<rsnano_core::Amount>;
    fn put(&self, write: &mut W, rep: rsnano_core::PublicKey, weight: rsnano_core::Amount);
    fn del(&self, write: &mut W, rep: &rsnano_core::PublicKey);
    fn count(&self, read: &R) -> u64;
}

pub trait PrunedStore<R: ReadTxnLike, W: WriteTxnLike> {
    fn count(&self, read: &R) -> u64;
    fn exists(&self, read: &R, hash: &rsnano_core::BlockHash) -> bool;
    fn put(&self, write: &mut W, hash: &rsnano_core::BlockHash);
    fn del(&self, write: &mut W, hash: &rsnano_core::BlockHash);
}

pub trait BlockStore<R: ReadTxnLike, W: WriteTxnLike> {
    fn exists(&self, read: &R, hash: &rsnano_core::BlockHash) -> bool;
    fn get(&self, read: &R, hash: &rsnano_core::BlockHash) -> Option<rsnano_core::SavedBlock>;
    fn del(&self, write: &mut W, hash: &rsnano_core::BlockHash);
}
