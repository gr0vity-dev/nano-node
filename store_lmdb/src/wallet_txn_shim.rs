use std::{any::Any, time::Duration};

use rsnano_nullable_lmdb::{
    LmdbDatabase, ReadTransaction, RoCursor, Transaction as LmdbTransaction, WriteFlags,
    WriteTransaction,
};
use store_traits::transaction::{WalletReadTxn, WalletWriteTxn};

/// Temporary shim over an LMDB read transaction. Provides only the operations
/// wallet code needs while keeping LMDB types quarantined.
pub struct WalletReadTxnSHIM {
    inner: ReadTransaction,
}

impl WalletReadTxnSHIM {
    pub fn new(inner: ReadTransaction) -> Self {
        Self { inner }
    }

    pub(crate) fn inner(&self) -> &ReadTransaction {
        &self.inner
    }

    pub fn commit(self) {
        self.inner.commit();
    }

    pub fn refresh(self) -> Self {
        Self {
            inner: self.inner.refresh(),
        }
    }

    pub fn get(&self, database: LmdbDatabase, key: &[u8]) -> rsnano_nullable_lmdb::Result<&[u8]> {
        self.inner.get(database, key)
    }

    pub fn open_ro_cursor(
        &self,
        database: LmdbDatabase,
    ) -> rsnano_nullable_lmdb::Result<RoCursor<'_>> {
        self.inner.open_ro_cursor(database)
    }

    pub fn count(&self, database: LmdbDatabase) -> u64 {
        self.inner.count(database)
    }

    pub fn into_inner(self) -> ReadTransaction {
        self.inner
    }
}

impl LmdbTransaction for WalletReadTxnSHIM {
    fn is_refresh_needed(&self) -> bool {
        self.inner.is_refresh_needed()
    }

    fn is_refresh_needed_with(&self, max_duration: Duration) -> bool {
        self.inner.is_refresh_needed_with(max_duration)
    }

    fn get(&self, database: LmdbDatabase, key: &[u8]) -> rsnano_nullable_lmdb::Result<&[u8]> {
        self.inner.get(database, key)
    }

    fn open_ro_cursor(&self, database: LmdbDatabase) -> rsnano_nullable_lmdb::Result<RoCursor<'_>> {
        self.inner.open_ro_cursor(database)
    }

    fn count(&self, database: LmdbDatabase) -> u64 {
        self.inner.count(database)
    }
}

/// Temporary shim over an LMDB write transaction.
pub struct WalletWriteTxnSHIM {
    inner: WriteTransaction,
}

impl WalletWriteTxnSHIM {
    pub fn new(inner: WriteTransaction) -> Self {
        Self { inner }
    }

    pub(crate) fn inner(&self) -> &WriteTransaction {
        &self.inner
    }

    pub(crate) fn inner_mut(&mut self) -> &mut WriteTransaction {
        &mut self.inner
    }

    pub fn commit(self) {
        self.inner.commit();
    }

    pub fn put(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: &[u8],
        flags: WriteFlags,
    ) -> rsnano_nullable_lmdb::Result<()> {
        self.inner.put(database, key, value, flags)
    }

    pub fn delete(
        &mut self,
        database: LmdbDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> rsnano_nullable_lmdb::Result<()> {
        self.inner.delete(database, key, value)
    }

    pub fn clear_db(&mut self, database: LmdbDatabase) -> rsnano_nullable_lmdb::Result<()> {
        self.inner.clear_db(database)
    }

    pub fn into_inner(self) -> WriteTransaction {
        self.inner
    }
}

impl LmdbTransaction for WalletWriteTxnSHIM {
    fn is_refresh_needed(&self) -> bool {
        self.inner.is_refresh_needed()
    }

    fn is_refresh_needed_with(&self, max_duration: Duration) -> bool {
        self.inner.is_refresh_needed_with(max_duration)
    }

    fn get(&self, database: LmdbDatabase, key: &[u8]) -> rsnano_nullable_lmdb::Result<&[u8]> {
        self.inner.get(database, key)
    }

    fn open_ro_cursor(&self, database: LmdbDatabase) -> rsnano_nullable_lmdb::Result<RoCursor<'_>> {
        self.inner.open_ro_cursor(database)
    }

    fn count(&self, database: LmdbDatabase) -> u64 {
        self.inner.count(database)
    }
}

/// Temporary trait alias letting wallet logic accept either shim without naming LMDB types.
impl WalletReadTxn for WalletReadTxnSHIM {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn commit(self: Box<Self>) {
        WalletReadTxnSHIM::commit(*self);
    }
}

impl WalletReadTxn for WalletWriteTxnSHIM {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn commit(self: Box<Self>) {
        WalletWriteTxnSHIM::commit(*self);
    }
}

impl WalletWriteTxn for WalletWriteTxnSHIM {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn downcast_read<T: 'static>(txn: &dyn WalletReadTxn) -> Option<&T> {
    txn.as_any().downcast_ref::<T>()
}

pub(crate) fn wallet_lmdb_read_txn(txn: &dyn WalletReadTxn) -> &dyn LmdbTransaction {
    if let Some(shim) = downcast_read::<WalletReadTxnSHIM>(txn) {
        shim.inner()
    } else if let Some(shim) = downcast_read::<WalletWriteTxnSHIM>(txn) {
        shim.inner()
    } else if let Some(raw) = downcast_read::<ReadTransaction>(txn) {
        raw
    } else if let Some(raw) = downcast_read::<WriteTransaction>(txn) {
        raw
    } else {
        panic!("Unsupported wallet read transaction type");
    }
}

pub(crate) fn wallet_lmdb_write_txn(txn: &mut dyn WalletWriteTxn) -> &mut WriteTransaction {
    if txn.as_any().is::<WalletWriteTxnSHIM>() {
        let shim = txn
            .as_any_mut()
            .downcast_mut::<WalletWriteTxnSHIM>()
            .expect("wallet txn shim downcast failed");
        shim.inner_mut()
    } else {
        txn.as_any_mut()
            .downcast_mut::<WriteTransaction>()
            .expect("Unsupported wallet write transaction type")
    }
}
