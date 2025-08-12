pub trait TransactionLike {
    fn is_refresh_needed(&self) -> bool;
    fn as_any(&self) -> &dyn std::any::Any;
}

pub trait ReadTxnLike: TransactionLike {}

pub trait WriteTxnLike: TransactionLike {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

pub trait VersionStore {
    fn get(&self, read: &dyn ReadTxnLike) -> Option<i32>;
    fn set(&self, write: &mut dyn WriteTxnLike, version: i32);
}

pub trait StoreProvider {
    type ReadTxn: ReadTxnLike;
    type WriteTxn: WriteTxnLike;

    fn begin_read(&self) -> Self::ReadTxn;
    fn begin_write(&self) -> Self::WriteTxn;
    fn refresh(&self, write: Self::WriteTxn) -> Self::WriteTxn;
    fn commit(&self, write: Self::WriteTxn);

    fn version(&self) -> &dyn VersionStore;
}
