use std::ops::Bound;

use rsnano_types::{
    Account, AccountInfo, Amount, BlockHash, ConfirmationHeightInfo, PendingInfo, PendingKey,
    SavedBlock,
};

use super::{AnyReceivableIterator, LedgerSet};

use crate::{LedgerStore, RangeBounds as StoreRangeBounds};
use store_traits::LedgerReadTxn;

pub trait ConfirmedSet: LedgerSet {
    fn get_block(&self, hash: &BlockHash) -> Option<SavedBlock>;
    fn get_conf_info(&self, account: &Account) -> Option<ConfirmationHeightInfo>;
}

/// Only blocks that are confirmed.
/// It owns the DB transaction
pub struct OwningConfirmedSet<'a> {
    store: &'a dyn LedgerStore,
    tx: Box<dyn LedgerReadTxn>,
}

impl<'a> OwningConfirmedSet<'a> {
    pub fn new(store: &'a dyn LedgerStore, tx: Box<dyn LedgerReadTxn>) -> Self {
        Self { store, tx }
    }

    fn borrowing_set(&'a self) -> BorrowingConfirmedSet<'a> {
        BorrowingConfirmedSet {
            store: self.store,
            tx: self.tx.as_ref(),
        }
    }

    fn first_receivable_lower_bound(
        &self,
        account: Account,
        send_hash: BlockHash,
    ) -> Option<(PendingKey, PendingInfo)> {
        let start = PendingKey::new(account, send_hash);
        let mut it = self.store.pending().iter_range(
            self.tx.as_ref(),
            StoreRangeBounds::new(Bound::Included(start), Bound::Unbounded),
        );

        let (mut key, mut info) = it.next()?;

        while !self.block_exists(&key.send_block_hash) {
            (key, info) = it.next()?;
        }

        Some((key, info))
    }

    /// Returns the next receivable entry for an account greater than or equal to 'account'
    pub fn receivable_lower_bound<'txn>(
        &'a self,
        account: Account,
    ) -> ConfirmedReceivableIterator<'txn>
    where
        'a: 'txn,
    {
        ConfirmedReceivableIterator::<'txn> {
            set: self,
            requested_account: account,
            actual_account: None,
            next_hash: Some(BlockHash::ZERO),
        }
    }

    pub fn frontiers(&self) -> impl Iterator<Item = (Account, BlockHash)> {
        self.store
            .confirmation_height()
            .iter(self.tx.as_ref())
            .map(|(account, conf_info)| (account, conf_info.frontier))
    }
}

impl<'a> LedgerSet for OwningConfirmedSet<'a> {
    fn block_exists(&self, hash: &BlockHash) -> bool {
        self.borrowing_set().block_exists(hash)
    }

    fn account_receivable(&self, account: &Account) -> Amount {
        self.borrowing_set().account_receivable(account)
    }

    fn account_balance(&self, account: &Account) -> Amount {
        self.borrowing_set().account_balance(account)
    }

    fn get_account(&self, account: &Account) -> Option<AccountInfo> {
        self.borrowing_set().get_account(account)
    }
}

impl<'a> ConfirmedSet for OwningConfirmedSet<'a> {
    fn get_block(&self, hash: &BlockHash) -> Option<SavedBlock> {
        self.borrowing_set().get_block(hash)
    }

    fn get_conf_info(&self, account: &Account) -> Option<ConfirmationHeightInfo> {
        self.borrowing_set().get_conf_info(account)
    }
}

/// Only blocks that are confirmed.
/// It borrows the DB transaction
pub struct BorrowingConfirmedSet<'a> {
    store: &'a dyn LedgerStore,
    tx: &'a dyn LedgerReadTxn,
}

impl<'a> BorrowingConfirmedSet<'a> {
    pub fn new(store: &'a dyn LedgerStore, tx: &'a dyn LedgerReadTxn) -> Self {
        Self { store, tx }
    }

    /// Returns the next receivable entry for the account 'account' with hash greater than 'hash'
    fn account_receivable_upper_bound<'txn>(
        &self,
        account: Account,
        hash: BlockHash,
    ) -> AnyReceivableIterator<'txn>
    where
        'a: 'txn,
    {
        AnyReceivableIterator::<'txn>::new(
            self.tx,
            self.store.pending(),
            account,
            Some(account),
            hash.inc(),
        )
    }

    fn account_head(&self, account: &Account) -> Option<BlockHash> {
        let info = self.store.confirmation_height().get(self.tx, account)?;
        Some(info.frontier)
    }
}

impl<'a> LedgerSet for BorrowingConfirmedSet<'a> {
    fn block_exists(&self, hash: &BlockHash) -> bool {
        self.get_block(hash).is_some()
    }

    fn account_receivable(&self, account: &Account) -> Amount {
        let mut result = Amount::ZERO;

        for (key, info) in self.account_receivable_upper_bound(*account, BlockHash::ZERO) {
            if self.block_exists(&key.send_block_hash) {
                result += info.amount;
            }
        }

        result
    }

    fn account_balance(&self, account: &Account) -> Amount {
        let Some(head) = self.account_head(account) else {
            return Amount::ZERO;
        };

        self.get_block(&head)
            .map(|b| b.balance())
            .unwrap_or_default()
    }

    fn get_account(&self, _account: &Account) -> Option<AccountInfo> {
        unimplemented!()
    }
}

impl<'a> ConfirmedSet for BorrowingConfirmedSet<'a> {
    fn get_block(&self, hash: &BlockHash) -> Option<SavedBlock> {
        if hash.is_zero() {
            return None;
        }
        let block = self.store.block().get(self.tx, hash)?;

        let conf_info = self
            .store
            .confirmation_height()
            .get(self.tx, &block.account())?;

        if block.height() <= conf_info.height {
            Some(block)
        } else {
            None
        }
    }

    fn get_conf_info(&self, account: &Account) -> Option<ConfirmationHeightInfo> {
        self.store.confirmation_height().get(self.tx, account)
    }
}

pub struct ConfirmedReceivableIterator<'a> {
    pub set: &'a OwningConfirmedSet<'a>,
    pub requested_account: Account,
    pub actual_account: Option<Account>,
    pub next_hash: Option<BlockHash>,
}

impl<'a> Iterator for ConfirmedReceivableIterator<'a> {
    type Item = (PendingKey, PendingInfo);

    fn next(&mut self) -> Option<Self::Item> {
        let hash = self.next_hash?;
        let account = self.actual_account.unwrap_or(self.requested_account);
        let (key, info) = self.set.first_receivable_lower_bound(account, hash)?;
        match self.actual_account {
            Some(account) => {
                if key.receiving_account == account {
                    self.next_hash = key.send_block_hash.inc();
                    Some((key.clone(), info.clone()))
                } else {
                    None
                }
            }
            None => {
                self.actual_account = Some(key.receiving_account);
                self.next_hash = key.send_block_hash.inc();
                Some((key.clone(), info.clone()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Ledger;
    use rsnano_types::{
        Account, AccountInfo, BlockHash, ConfirmationHeightInfo, PendingInfo, PendingKey,
        SavedBlock,
    };

    #[test]
    fn iter_receivables() {
        let account = Account::from(1);

        let block1 = SavedBlock::new_test_instance_with_key(42);
        let block2 = SavedBlock::new_test_instance_with_key(43);
        let block3 = SavedBlock::new_test_instance_with_key(44);

        let ledger = Ledger::new_null_builder(rsnano_store_lmdb::null_ledger_store_factory())
            .blocks([&block1, &block2, &block3])
            .confirmation_height(
                &block1.account(),
                &ConfirmationHeightInfo::new(9999, BlockHash::ZERO),
            )
            .confirmation_height(
                &block2.account(),
                &ConfirmationHeightInfo::new(0, BlockHash::ZERO),
            )
            .confirmation_height(
                &block3.account(),
                &ConfirmationHeightInfo::new(9999, BlockHash::ZERO),
            )
            .pending(
                &PendingKey::new(account, block1.hash()),
                &PendingInfo::new_test_instance(),
            )
            .pending(
                &PendingKey::new(account, block2.hash()),
                &PendingInfo::new_test_instance(),
            )
            .pending(
                &PendingKey::new(account, block3.hash()),
                &PendingInfo::new_test_instance(),
            )
            .finish();

        let confirmed = ledger.confirmed();
        let receivable: Vec<_> = confirmed
            .receivable_lower_bound(Account::ZERO)
            .map(|i| i.0)
            .collect();

        let mut expected = vec![
            PendingKey::new(account, block1.hash()),
            PendingKey::new(account, block3.hash()),
        ];
        expected.sort_by_key(|i| i.send_block_hash);

        assert_eq!(receivable, expected);
    }

    #[test]
    fn iter_frontiers() {
        let account1 = Account::from(1);
        let account2 = Account::from(2);
        let account3 = Account::from(3);

        let hash1 = BlockHash::from(100);
        let hash2 = BlockHash::from(200);
        let hash3 = BlockHash::from(300);

        let ledger = Ledger::new_null_builder(rsnano_store_lmdb::null_ledger_store_factory())
            .account_info(&account1, &AccountInfo::new_test_instance())
            .account_info(&account2, &AccountInfo::new_test_instance())
            .account_info(&account3, &AccountInfo::new_test_instance())
            .confirmation_height(&account1, &ConfirmationHeightInfo::new(0, hash1))
            .confirmation_height(&account2, &ConfirmationHeightInfo::new(0, hash2))
            .confirmation_height(&account3, &ConfirmationHeightInfo::new(0, hash3))
            .finish();

        assert_eq!(
            ledger.confirmed().frontiers().collect::<Vec<_>>(),
            vec![(account1, hash1), (account2, hash2), (account3, hash3)]
        );
    }
}
