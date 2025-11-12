pub trait LedgerReadTxn {
    /// Temporary LMDB escape hatch until adapters are in place.
    fn as_lmdb_txn_shim(&self) -> &dyn rsnano_nullable_lmdb::Transaction;
}

pub trait LedgerWriteTxn: LedgerReadTxn {
    fn as_lmdb_write_txn_shim(&mut self) -> &mut rsnano_nullable_lmdb::WriteTransaction;
}

impl LedgerReadTxn for rsnano_nullable_lmdb::ReadTransaction {
    fn as_lmdb_txn_shim(&self) -> &dyn rsnano_nullable_lmdb::Transaction {
        self
    }
}

impl LedgerReadTxn for rsnano_nullable_lmdb::WriteTransaction {
    fn as_lmdb_txn_shim(&self) -> &dyn rsnano_nullable_lmdb::Transaction {
        self
    }
}

impl LedgerWriteTxn for rsnano_nullable_lmdb::WriteTransaction {
    fn as_lmdb_write_txn_shim(&mut self) -> &mut rsnano_nullable_lmdb::WriteTransaction {
        self
    }
}

pub trait WalletReadTxn {
    fn as_lmdb_txn_shim(&self) -> &dyn rsnano_nullable_lmdb::Transaction;
}

pub trait WalletWriteTxn: WalletReadTxn {
    fn as_lmdb_write_txn_shim(&mut self) -> &mut rsnano_nullable_lmdb::WriteTransaction;
}

impl WalletReadTxn for rsnano_nullable_lmdb::ReadTransaction {
    fn as_lmdb_txn_shim(&self) -> &dyn rsnano_nullable_lmdb::Transaction {
        self
    }
}

impl WalletReadTxn for rsnano_nullable_lmdb::WriteTransaction {
    fn as_lmdb_txn_shim(&self) -> &dyn rsnano_nullable_lmdb::Transaction {
        self
    }
}

impl WalletWriteTxn for rsnano_nullable_lmdb::WriteTransaction {
    fn as_lmdb_write_txn_shim(&mut self) -> &mut rsnano_nullable_lmdb::WriteTransaction {
        self
    }
}
