pub trait LedgerReadTxn: rsnano_nullable_lmdb::Transaction {}
pub trait LedgerWriteTxn: LedgerReadTxn {}

impl LedgerReadTxn for rsnano_nullable_lmdb::ReadTransaction {}
impl LedgerReadTxn for rsnano_nullable_lmdb::WriteTransaction {}
impl LedgerWriteTxn for rsnano_nullable_lmdb::WriteTransaction {}

pub trait WalletReadTxn: rsnano_nullable_lmdb::Transaction {}
pub trait WalletWriteTxn: WalletReadTxn {}

impl WalletReadTxn for rsnano_nullable_lmdb::ReadTransaction {}
impl WalletReadTxn for rsnano_nullable_lmdb::WriteTransaction {}
impl WalletWriteTxn for rsnano_nullable_lmdb::WriteTransaction {}
