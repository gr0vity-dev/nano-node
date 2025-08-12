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

    fn begin_read(&self) -> Self::ReadTxn;
    fn begin_write(&self) -> Self::WriteTxn;
    fn refresh(&self, write: Self::WriteTxn) -> Self::WriteTxn;
    fn commit(&self, write: Self::WriteTxn);

    fn version(&self) -> &Self::Version;
}
