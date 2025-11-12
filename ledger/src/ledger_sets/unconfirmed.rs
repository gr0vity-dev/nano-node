use rsnano_types::{Account, AccountInfo, Amount, BlockHash};

use super::LedgerSet;
use crate::{LedgerReadTxnSHIM, LedgerStore};

/// Unconfirmed Blocks of the ledger.
/// It owns the DB transaction
pub(crate) struct OwningUnconfirmedSet<'a> {
    store: &'a dyn LedgerStore,
    tx: LedgerReadTxnSHIM,
}

impl<'a> OwningUnconfirmedSet<'a> {
    pub fn new(store: &'a dyn LedgerStore, tx: LedgerReadTxnSHIM) -> Self {
        Self { store, tx }
    }

    fn borrowing_set(&'a self) -> BorrowingUnconfirmedSet<'a> {
        BorrowingUnconfirmedSet {
            store: self.store,
            tx: &self.tx,
        }
    }
}

impl<'a> LedgerSet for OwningUnconfirmedSet<'a> {
    fn block_exists(&self, hash: &BlockHash) -> bool {
        self.borrowing_set().block_exists(hash)
    }

    fn account_receivable(&self, account: &Account) -> Amount {
        self.borrowing_set().account_receivable(account)
    }

    fn account_balance(&self, account: &Account) -> Amount {
        self.borrowing_set().account_receivable(account)
    }

    fn get_account(&self, account: &Account) -> Option<AccountInfo> {
        self.borrowing_set().get_account(account)
    }
}

/// Unconfirmed Blocks of the ledger
/// It borrows the DB transaction
pub(crate) struct BorrowingUnconfirmedSet<'a> {
    store: &'a dyn LedgerStore,
    tx: &'a LedgerReadTxnSHIM,
}

impl<'a> LedgerSet for BorrowingUnconfirmedSet<'a> {
    fn block_exists(&self, hash: &BlockHash) -> bool {
        if hash.is_zero() {
            return false;
        }

        let Some(block) = self.store.block().get(&*self.tx, hash) else {
            return false;
        };

        let conf_info = self
            .store
            .confirmation_height()
            .get(&*self.tx, &block.account())
            .unwrap_or_default();

        block.height() > conf_info.height
    }

    fn account_receivable(&self, _account: &Account) -> Amount {
        unimplemented!()
    }

    fn account_balance(&self, _account: &Account) -> Amount {
        unimplemented!()
    }

    fn get_account(&self, _account: &Account) -> Option<AccountInfo> {
        unimplemented!()
    }
}
