use std::ops::{RangeBounds, RangeFrom};

use rsnano_core::{
    block_priority, utils::BlockPriority, Account, AccountInfo, Amount, Block, BlockHash,
    DependentBlocks, DetailedBlock, PendingInfo, PendingKey, PublicKey, QualifiedRoot, Root,
    SavedBlock,
};
use rsnano_nullable_lmdb::{ReadTransaction, Transaction};
use rsnano_store_lmdb::{LmdbPendingStore, LmdbRangeIterator, LmdbStore};
use store_api::{BlockStore, FinalVoteStore, PendingStore, PrunedStore, StoreProvider, SuccessorStore};

use super::{BorrowingConfirmedSet, ConfirmedSet, LedgerSet};
use crate::{DependentBlocksFinder, LedgerConstants, RepresentativeBlockFinder};

pub trait AnySet: LedgerSet {
    fn should_refresh(&self) -> bool;
    fn block_exists_or_pruned(&self, hash: &BlockHash) -> bool;
    fn get_block(&self, hash: &BlockHash) -> Option<SavedBlock>;
    fn receivable_exists(&self, account: Account) -> bool;
    fn confirmed(&self) -> BorrowingConfirmedSet;

    fn block_balance(&self, hash: &BlockHash) -> Option<Amount> {
        if hash.is_zero() {
            return None;
        }

        self.get_block(hash).map(|b| b.balance())
    }

    fn dependent_blocks(&self, block: &SavedBlock) -> DependentBlocks;
    fn dependents_confirmed(&self, block: &SavedBlock) -> bool;
    fn dependents_confirmed_for_unsaved_block(&self, block: &Block) -> bool;
    fn block_successor(&self, hash: &BlockHash) -> Option<BlockHash>;
    fn block_successor_by_qualified_root(&self, root: &QualifiedRoot) -> Option<BlockHash>;

    /// Returned priority balance is maximum of block balance and previous block balance
    /// to handle full account balance send cases.
    /// Returned timestamp is the previous block timestamp or the current timestamp
    /// if there's no previous block.
    fn block_priority(&self, block: &SavedBlock) -> BlockPriority;

    fn previous_block(&self, block: &SavedBlock) -> Option<SavedBlock>;
    fn get_pending(&self, key: &PendingKey) -> Option<PendingInfo>;
    fn account_head(&self, account: &Account) -> Option<BlockHash>;
    fn block_account(&self, hash: &BlockHash) -> Option<Account>;

    /// Returns the latest block with representative information
    fn representative_block_hash(&self, hash: &BlockHash) -> BlockHash;

    /// Given the block hash of a send block, find the associated receive block that receives that send.
    /// The send block hash is not checked in any way, it is assumed to be correct.
    /// Return the receive block on success and None on failure
    fn find_receive_block_by_send_hash(
        &self,
        destination: &Account,
        send_block_hash: &BlockHash,
    ) -> Option<SavedBlock>;

    fn linked_account(&self, block: &SavedBlock) -> Option<Account>;

    fn block_amount(&self, hash: &BlockHash) -> Option<Amount>;
    fn block_amount_for(&self, block: &SavedBlock) -> Option<Amount>;
    fn detailed_block(&self, hash: &BlockHash) -> Option<DetailedBlock>;

    /// Returns the next receivable entry for an account greater than 'account'
    fn receivable_upper_bound(&self, account: Account) -> AnyReceivableIterator;

    /// Returns the next receivable entry for an account greater than or equal to 'account'
    fn receivable_lower_bound(&self, account: Account) -> AnyReceivableIterator;

    /// Returns the next receivable entry for the account 'account' with hash greater than 'hash'
    fn account_receivable_upper_bound(
        &self,
        account: Account,
        hash: BlockHash,
    ) -> AnyReceivableIterator;

    fn get_final_vote(&self, root: &QualifiedRoot) -> Option<BlockHash>;
}

/// All blocks - either confirmed or unconfirmed
/// It owns the DB transaction
pub struct OwningAnySet<'a> {
    store: &'a LmdbStore,
    txn: ReadTransaction,
    constants: &'a LedgerConstants,
}

impl<'a> OwningAnySet<'a> {
    pub(crate) fn new(store: &'a LmdbStore, constants: &'a LedgerConstants) -> Self {
        let tx = store.begin_read();
        Self {
            store,
            txn: tx,
            constants,
        }
    }

    fn borrowing_set(&'a self) -> BorrowingAnySet<'a> {
        BorrowingAnySet {
            store: self.store,
            tx: &self.txn,
            constants: self.constants,
        }
    }

    pub fn accounts_range(
        &self,
        range: impl RangeBounds<Account> + 'static,
    ) -> Box<dyn Iterator<Item = (Account, AccountInfo)> + '_> {
        self.store.account.iter_range(&self.txn, range)
    }

    pub fn iter_accounts(&self) -> impl Iterator<Item = (Account, AccountInfo)> + '_ {
        self.store.account.iter(&self.txn)
    }

    pub fn iter_account_range(
        &self,
        range: impl RangeBounds<Account> + 'static,
    ) -> Box<dyn Iterator<Item = (Account, AccountInfo)> + '_> {
        self.store.account.iter_range(&self.txn, range)
    }

    pub fn iter_pending_range(
        &self,
        range: impl RangeBounds<PendingKey> + 'static,
    ) -> impl Iterator<Item = (PendingKey, PendingInfo)> + '_ {
        self.store.pending.iter_range(&self.txn, range)
    }

    pub fn random_blocks(&self, count: usize) -> Vec<SavedBlock> {
        let mut result = Vec::with_capacity(count);
        let starting_hash = BlockHash::random();

        // It is more efficient to choose a random starting point and pick a few sequential blocks from there
        let mut it = self.store.block.iter_range(&self.txn, starting_hash..);
        while result.len() < count {
            match it.next() {
                Some(block) => result.push(block),
                None => {
                    // Wrap around when reaching the end
                    it = self.store.block.iter_range(&self.txn, BlockHash::zero()..);
                }
            }
        }

        result
    }

    /// Return latest root for account, account number if there are no blocks for this account
    pub fn latest_root(&self, account: &Account) -> Root {
        match self.get_account(account) {
            Some(info) => info.head.into(),
            None => account.into(),
        }
    }

    /// Returns the exact vote weight for the given representative by doing a database lookup
    pub fn weight_exact(&self, representative: PublicKey) -> Amount {
        self.store
            .rep_weight
            .get(&self.txn, &representative)
            .unwrap_or_default()
    }

    pub fn is_refresh_needed(&self) -> bool {
        self.txn.is_refresh_needed()
    }

    pub fn refresh(self) -> Self {
        Self {
            store: self.store,
            txn: self.txn.refresh(),
            constants: self.constants,
        }
    }
}

impl<'a> LedgerSet for OwningAnySet<'a> {
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

impl<'a> AnySet for OwningAnySet<'a> {
    fn should_refresh(&self) -> bool {
        self.borrowing_set().should_refresh()
    }
    fn block_exists_or_pruned(&self, hash: &BlockHash) -> bool {
        self.borrowing_set().block_exists_or_pruned(hash)
    }

    fn get_block(&self, hash: &BlockHash) -> Option<SavedBlock> {
        self.borrowing_set().get_block(hash)
    }

    fn confirmed(&self) -> BorrowingConfirmedSet {
        BorrowingConfirmedSet::new(self.store, &self.txn)
    }

    fn dependent_blocks(&self, block: &SavedBlock) -> DependentBlocks {
        self.borrowing_set().dependent_blocks(block)
    }

    fn dependents_confirmed(&self, block: &SavedBlock) -> bool {
        self.borrowing_set().dependents_confirmed(block)
    }

    fn dependents_confirmed_for_unsaved_block(&self, block: &Block) -> bool {
        self.borrowing_set()
            .dependents_confirmed_for_unsaved_block(block)
    }

    fn block_successor(&self, hash: &BlockHash) -> Option<BlockHash> {
        self.borrowing_set().block_successor(hash)
    }

    fn block_successor_by_qualified_root(&self, root: &QualifiedRoot) -> Option<BlockHash> {
        self.borrowing_set().block_successor_by_qualified_root(root)
    }

    fn block_priority(&self, block: &SavedBlock) -> BlockPriority {
        self.borrowing_set().block_priority(block)
    }

    fn previous_block(&self, block: &SavedBlock) -> Option<SavedBlock> {
        self.borrowing_set().previous_block(block)
    }

    fn receivable_exists(&self, account: Account) -> bool {
        self.borrowing_set().receivable_exists(account)
    }

    fn get_pending(&self, key: &PendingKey) -> Option<PendingInfo> {
        self.borrowing_set().get_pending(key)
    }

    fn account_head(&self, account: &Account) -> Option<BlockHash> {
        self.borrowing_set().account_head(account)
    }

    fn block_account(&self, hash: &BlockHash) -> Option<Account> {
        self.borrowing_set().block_account(hash)
    }

    fn representative_block_hash(&self, hash: &BlockHash) -> BlockHash {
        self.borrowing_set().representative_block_hash(hash)
    }

    fn find_receive_block_by_send_hash(
        &self,
        destination: &Account,
        send_block_hash: &BlockHash,
    ) -> Option<SavedBlock> {
        self.borrowing_set()
            .find_receive_block_by_send_hash(destination, send_block_hash)
    }

    fn linked_account(&self, block: &SavedBlock) -> Option<Account> {
        self.borrowing_set().linked_account(block)
    }

    fn block_amount(&self, hash: &BlockHash) -> Option<Amount> {
        self.borrowing_set().block_amount(hash)
    }

    fn block_amount_for(&self, block: &SavedBlock) -> Option<Amount> {
        self.borrowing_set().block_amount_for(block)
    }

    fn detailed_block(&self, hash: &BlockHash) -> Option<DetailedBlock> {
        self.borrowing_set().detailed_block(hash)
    }

    fn receivable_upper_bound(&self, account: Account) -> AnyReceivableIterator {
        match account.inc() {
            None => AnyReceivableIterator::new(
                &self.txn,
                &self.store.pending,
                Default::default(),
                None,
                None,
            ),
            Some(account) => AnyReceivableIterator::new(
                &self.txn,
                &self.store.pending,
                account,
                None,
                Some(BlockHash::zero()),
            ),
        }
    }

    fn receivable_lower_bound(&self, account: Account) -> AnyReceivableIterator {
        AnyReceivableIterator::new(
            &self.txn,
            &self.store.pending,
            account,
            None,
            Some(BlockHash::zero()),
        )
    }

    fn account_receivable_upper_bound(
        &self,
        account: Account,
        hash: BlockHash,
    ) -> AnyReceivableIterator {
        AnyReceivableIterator::new(
            &self.txn,
            &self.store.pending,
            account,
            Some(account),
            hash.inc(),
        )
    }

    fn get_final_vote(&self, root: &QualifiedRoot) -> Option<BlockHash> {
        self.borrowing_set().get_final_vote(root)
    }
}

pub(crate) struct BorrowingAnySet<'a> {
    pub constants: &'a LedgerConstants,
    pub store: &'a LmdbStore,
    pub tx: &'a dyn Transaction,
}

impl<'a> BorrowingAnySet<'a> {
    fn dependent_blocks_for_unsaved_block(&self, block: &Block) -> DependentBlocks {
        DependentBlocksFinder::new(self, &self.constants)
            .find_dependent_blocks_for_unsaved_block(block)
    }
}

impl<'a> LedgerSet for BorrowingAnySet<'a> {
    fn block_exists(&self, hash: &BlockHash) -> bool {
        if hash.is_zero() {
            return false;
        }
        self.store.block.exists(self.tx, hash)
    }

    fn account_receivable(&self, account: &Account) -> Amount {
        let mut result = Amount::zero();

        for (_, info) in self.account_receivable_upper_bound(*account, BlockHash::zero()) {
            result += info.amount;
        }

        result
    }

    fn account_balance(&self, account: &Account) -> Amount {
        let Some(head) = self.account_head(account) else {
            return Amount::zero();
        };

        self.get_block(&head)
            .map(|b| b.balance())
            .unwrap_or_default()
    }

    fn get_account(&self, account: &Account) -> Option<AccountInfo> {
        self.store.account.get(self.tx, account)
    }
}

impl<'a> AnySet for BorrowingAnySet<'a> {
    fn get_block(&self, hash: &BlockHash) -> Option<SavedBlock> {
        if hash.is_zero() {
            return None;
        }
        self.store.block.get(self.tx, hash)
    }

    fn block_exists_or_pruned(&self, hash: &BlockHash) -> bool {
        if hash.is_zero() {
            return false;
        }
        if self.store.pruned.exists(self.tx, hash) {
            true
        } else {
            self.block_exists(hash)
        }
    }

    fn receivable_exists(&self, account: Account) -> bool {
        self.account_receivable_upper_bound(account, BlockHash::zero())
            .next()
            .is_some()
    }

    fn confirmed(&self) -> BorrowingConfirmedSet {
        BorrowingConfirmedSet::new(self.store, self.tx)
    }

    fn dependents_confirmed_for_unsaved_block(&self, block: &Block) -> bool {
        self.dependent_blocks_for_unsaved_block(block)
            .iter()
            .all(|hash| self.confirmed().block_exists_or_pruned(hash))
    }

    fn dependents_confirmed(&self, block: &SavedBlock) -> bool {
        self.dependent_blocks(block)
            .iter()
            .all(|hash| self.confirmed().block_exists_or_pruned(hash))
    }

    fn dependent_blocks(&self, block: &SavedBlock) -> DependentBlocks {
        DependentBlocksFinder::new(self, self.constants).find_dependent_blocks(block)
    }

    fn should_refresh(&self) -> bool {
        self.tx.is_refresh_needed()
    }

    fn block_successor(&self, hash: &BlockHash) -> Option<BlockHash> {
        self.block_successor_by_qualified_root(&QualifiedRoot::new(hash.into(), *hash))
    }

    fn block_successor_by_qualified_root(&self, root: &QualifiedRoot) -> Option<BlockHash> {
        if !root.previous.is_zero() {
            <rsnano_store_lmdb::successor_store::LmdbSuccessorStore as SuccessorStore<
                rsnano_store_lmdb::adapter::ReadTxnPub,
                rsnano_store_lmdb::adapter::WriteTxnPub,
            >>::get(
                StoreProvider::successor(self.store),
                &StoreProvider::begin_read(self.store),
                &root.previous,
            )
        } else {
            self.get_account(&root.root.into()).map(|i| i.open_block)
        }
    }

    fn block_priority(&self, block: &SavedBlock) -> BlockPriority {
        let previous_block = self.previous_block(block);
        block_priority(block, previous_block.as_ref())
    }

    fn previous_block(&self, block: &SavedBlock) -> Option<SavedBlock> {
        if block.previous().is_zero() {
            None
        } else {
            self.get_block(&block.previous())
        }
    }

    fn get_pending(&self, key: &PendingKey) -> Option<PendingInfo> {
        self.store.pending.get(self.tx, key)
    }

    fn account_head(&self, account: &Account) -> Option<BlockHash> {
        self.get_account(account).map(|i| i.head)
    }

    fn block_account(&self, hash: &BlockHash) -> Option<Account> {
        self.get_block(hash).map(|b| b.account())
    }

    /// Returns the latest block with representative information
    fn representative_block_hash(&self, hash: &BlockHash) -> BlockHash {
        let hash = RepresentativeBlockFinder::new(self.store).find_rep_block(*hash);
        debug_assert!(hash.is_zero() || self.store.block.exists(self.tx, &hash));
        hash
    }

    fn find_receive_block_by_send_hash(
        &self,
        destination: &Account,
        send_block_hash: &BlockHash,
    ) -> Option<SavedBlock> {
        // get the cemented frontier
        let info = self.confirmed().get_conf_info(destination)?;
        let mut possible_receive_block = self.get_block(&info.frontier);

        // walk down the chain until the source field of a receive block matches the send block hash
        while let Some(current) = possible_receive_block {
            if current.is_receive() && Some(*send_block_hash) == current.source() {
                // we have a match
                return Some(current);
            }

            possible_receive_block = self.get_block(&current.previous());
        }

        None
    }

    fn linked_account(&self, block: &SavedBlock) -> Option<Account> {
        if block.is_send() {
            Some(block.destination_or_link())
        } else if block.is_receive() {
            self.block_account(&block.source_or_link())
        } else {
            None
        }
    }

    fn block_amount(&self, hash: &BlockHash) -> Option<Amount> {
        let block = self.get_block(hash)?;
        self.block_amount_for(&block)
    }

    fn block_amount_for(&self, block: &SavedBlock) -> Option<Amount> {
        let block_balance = block.balance();
        if block.previous().is_zero() {
            Some(block_balance)
        } else {
            let previous_balance = self.block_balance(&block.previous())?;
            if block_balance > previous_balance {
                Some(block_balance - previous_balance)
            } else {
                Some(previous_balance - block_balance)
            }
        }
    }

    fn detailed_block(&self, hash: &BlockHash) -> Option<DetailedBlock> {
        let block = self.get_block(hash)?;
        let amount = self.block_amount_for(&block);
        let confirmed = self.confirmed().block_exists_or_pruned(hash);
        Some(DetailedBlock {
            block,
            amount,
            confirmed,
        })
    }

    /// Returns the next receivable entry for an account greater than 'account'
    fn receivable_upper_bound(&self, account: Account) -> AnyReceivableIterator {
        match account.inc() {
            None => AnyReceivableIterator::new(
                self.tx,
                &self.store.pending,
                Default::default(),
                None,
                None,
            ),
            Some(account) => AnyReceivableIterator::new(
                self.tx,
                &self.store.pending,
                account,
                None,
                Some(BlockHash::zero()),
            ),
        }
    }

    fn receivable_lower_bound(&self, account: Account) -> AnyReceivableIterator {
        AnyReceivableIterator::new(
            self.tx,
            &self.store.pending,
            account,
            None,
            Some(BlockHash::zero()),
        )
    }

    fn account_receivable_upper_bound(
        &self,
        account: Account,
        hash: BlockHash,
    ) -> AnyReceivableIterator {
        AnyReceivableIterator::new(
            self.tx,
            &self.store.pending,
            account,
            Some(account),
            hash.inc(),
        )
    }

    fn get_final_vote(&self, root: &QualifiedRoot) -> Option<BlockHash> {
        <rsnano_store_lmdb::LmdbFinalVoteStore as FinalVoteStore<
            rsnano_store_lmdb::adapter::ReadTxnPub,
            rsnano_store_lmdb::adapter::WriteTxnPub,
        >>::get(
            StoreProvider::final_vote(self.store),
            &StoreProvider::begin_read(self.store),
            root,
        )
    }
}

pub struct AnyReceivableIterator<'a> {
    returned_account: Option<Account>,
    inner: LmdbRangeIterator<'a, PendingKey, PendingInfo, RangeFrom<PendingKey>>,
    is_first: bool,
}

impl<'a> AnyReceivableIterator<'a> {
    pub fn new(
        txn: &'a dyn Transaction,
        pending: &'a LmdbPendingStore,
        requested_account: Account,
        returned_account: Option<Account>,
        next_hash: Option<BlockHash>,
    ) -> Self {
        let cursor = txn
            .open_ro_cursor(pending.database())
            .expect("could not read from account store");

        let inner = match next_hash {
            Some(hash) => {
                let start = PendingKey::new(requested_account, hash);
                LmdbRangeIterator::new(cursor, start..)
            }
            None => LmdbRangeIterator::empty(PendingKey::default()..),
        };

        Self {
            returned_account,
            inner,
            is_first: true,
        }
    }
}

impl<'a> Iterator for AnyReceivableIterator<'a> {
    type Item = (PendingKey, PendingInfo);

    fn next(&mut self) -> Option<Self::Item> {
        if self.is_first {
            self.is_first = false;
            let (key, info) = self.inner.next()?;
            match self.returned_account {
                Some(returned_acc) => {
                    if returned_acc != key.receiving_account {
                        return None;
                    }
                }
                None => {
                    // The first result defines the returned account
                    self.returned_account = Some(key.receiving_account);
                }
            }
            return Some((key, info));
        }

        let (key, info) = self.inner.next()?;
        match self.returned_account {
            Some(account) => {
                if key.receiving_account == account {
                    Some((key, info))
                } else {
                    None
                }
            }
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ledger;

    #[test]
    fn iter_all_lower_bound() {
        let key1 = PendingKey::new(Account::from(1), BlockHash::from(100));
        let key2 = PendingKey::new(Account::from(1), BlockHash::from(101));
        let key3 = PendingKey::new(Account::from(3), BlockHash::from(4));

        test_lower_bound(
            &[key1.clone(), key2.clone(), key3.clone()],
            Account::from(0),
            &[key1.clone(), key2.clone()],
        );
        test_lower_bound(
            &[key1.clone(), key2.clone(), key3.clone()],
            Account::from(1),
            &[key1.clone(), key2.clone()],
        );
        test_lower_bound(
            &[key1.clone(), key2.clone(), key3.clone()],
            Account::from(3),
            &[key3.clone()],
        );
        test_lower_bound(
            &[key1.clone(), key2.clone(), key3.clone()],
            Account::from(4),
            &[],
        );
    }

    #[test]
    fn iter_all_upper_bound() {
        let key1 = PendingKey::new(Account::from(1), BlockHash::from(100));
        let key2 = PendingKey::new(Account::from(1), BlockHash::from(101));
        let key3 = PendingKey::new(Account::from(3), BlockHash::from(4));
        test_upper_bound(
            &[key1.clone(), key2.clone(), key3.clone()],
            Account::from(0),
            &[key1.clone(), key2.clone()],
        );
        test_upper_bound(
            &[key1.clone(), key2.clone(), key3.clone()],
            Account::from(1),
            &[key3.clone()],
        );
        test_upper_bound(
            &[key1.clone(), key2.clone(), key3.clone()],
            Account::from(4),
            &[],
        );
    }

    fn test_upper_bound(
        existing_keys: &[PendingKey],
        queried_account: Account,
        expected_result: &[PendingKey],
    ) {
        let ledger = ledger_with_pending_entries(existing_keys);
        let result: Vec<_> = ledger
            .any()
            .receivable_upper_bound(queried_account)
            .map(|(k, _)| k)
            .collect();

        assert_eq!(result, expected_result);
    }

    fn test_lower_bound(
        existing_keys: &[PendingKey],
        queried_account: Account,
        expected_result: &[PendingKey],
    ) {
        let ledger = ledger_with_pending_entries(existing_keys);
        let result: Vec<_> = ledger
            .any()
            .receivable_lower_bound(queried_account)
            .map(|(k, _)| k)
            .collect();

        assert_eq!(result, expected_result);
    }

    fn ledger_with_pending_entries(existing_keys: &[PendingKey]) -> Ledger {
        let info = PendingInfo::new_test_instance();
        let mut builder = Ledger::new_null_builder();
        for key in existing_keys {
            builder = builder.pending(key, &info);
        }
        builder.finish()
    }
}
