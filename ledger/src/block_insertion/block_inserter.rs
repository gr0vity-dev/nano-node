use std::sync::{Arc, atomic::Ordering};

use rsnano_types::{
    Account, AccountInfo, Amount, Block, BlockSideband, PendingInfo, PendingKey, SavedBlock,
};

use crate::{DeferredLedgerOperations, Ledger};
use store_traits::LedgerWriteTxn;

#[derive(Debug, PartialEq, Clone)]
pub struct BlockInsertInstructions {
    pub account: Account,
    pub old_account_info: AccountInfo,
    pub set_account_info: AccountInfo,
    pub delete_pending: Option<PendingKey>,
    pub insert_pending: Option<(PendingKey, PendingInfo)>,
    pub set_sideband: BlockSideband,
    pub is_epoch_block: bool,
}

/// Inserts a new block into the ledger
pub struct BlockInserter<'a> {
    ledger: &'a Ledger,
    txn: &'a mut dyn LedgerWriteTxn,
    block: &'a Block,
    instructions: &'a BlockInsertInstructions,
}

impl<'a> BlockInserter<'a> {
    pub fn new(
        ledger: &'a Ledger,
        txn: &'a mut dyn LedgerWriteTxn,
        block: &'a Block,
        instructions: &'a BlockInsertInstructions,
    ) -> Self {
        Self {
            ledger,
            txn,
            block,
            instructions,
        }
    }

    pub fn insert(
        &mut self,
        deferred: &mut DeferredLedgerOperations,
    ) -> (Option<SavedBlock>, bool, bool) {
        if self.account_changed_since_validation() {
            if let Some(existing_block) =
                self.ledger.store.block().get(self.txn, &self.block.hash())
            {
                self.ledger.record_duplicate_insert_event();
                return (Some(existing_block), false, true);
            }
            return (None, false, false);
        }

        let sideband = self.instructions.set_sideband.clone();
        let saved_block = SavedBlock::new(self.block.clone(), sideband);
        let already_exists = self
            .ledger
            .store
            .block()
            .exists(self.txn, &saved_block.hash());
        self.ledger.store.block().put(self.txn, &saved_block);
        if !saved_block.previous().is_zero() {
            self.ledger.store.successors().put(
                self.txn,
                &saved_block.previous(),
                &saved_block.hash(),
            );
        }
        self.update_account();
        self.delete_old_pending_info();
        self.insert_new_pending_info();
        self.update_representative_cache(deferred);
        if !already_exists {
            let store = Arc::clone(&self.ledger.store);
            let events = self.ledger.block_count_events_arc();
            self.txn.on_commit(Box::new(move || {
                store.cache().block_count.fetch_add(1, Ordering::SeqCst);
                events.record_insert();
            }));
        } else {
            self.ledger.record_duplicate_insert_event();
        }

        (Some(saved_block), !already_exists, already_exists)
    }

    fn account_changed_since_validation(&mut self) -> bool {
        let account_info = self.get_current_account_info();
        let account_changed_since_validation =
            account_info.head != self.instructions.old_account_info.head;
        account_changed_since_validation
    }

    fn get_current_account_info(&mut self) -> AccountInfo {
        let account_info = self
            .ledger
            .store
            .account()
            .get(self.txn, &self.instructions.account)
            .unwrap_or_default();
        account_info
    }

    fn update_account(&mut self) {
        self.ledger.update_account(
            self.txn,
            &self.instructions.account,
            &self.instructions.old_account_info,
            &self.instructions.set_account_info,
        );
    }

    fn delete_old_pending_info(&mut self) {
        if let Some(key) = &self.instructions.delete_pending {
            self.ledger.store.pending().del(self.txn, key);
        }
    }

    fn insert_new_pending_info(&mut self) {
        if let Some((key, info)) = &self.instructions.insert_pending {
            self.ledger.store.pending().put(self.txn, key, info);
        }
    }

    fn update_representative_cache(&mut self, deferred: &mut DeferredLedgerOperations) {
        if !self.instructions.old_account_info.head.is_zero() {
            // Move existing representation & add in amount delta
            deferred.add_rep_weight_dual(
                self.instructions.old_account_info.representative,
                Amount::ZERO.wrapping_sub(self.instructions.old_account_info.balance),
                self.instructions.set_account_info.representative,
                self.instructions.set_account_info.balance,
            );
        } else {
            // Add in amount delta only
            deferred.add_rep_weight(
                self.instructions.set_account_info.representative,
                self.instructions.set_account_info.balance,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommitDisposition, DeferredLedgerOperations, Ledger, NullLedgerBuilder};
    mod insertion_test_helpers {
        use crate as ledger_crate;
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/common/test_helpers.rs"
        ));
    }
    use insertion_test_helpers::{commit_block_txn, legacy_open_block_instructions};
    use rsnano_types::{BlockHash, Epoch, PublicKey, TestBlockBuilder, UnixTimestamp};
    use std::sync::Arc;
    use store_rocksdb::default_ledger_store_factory;
    use store_traits::ledger::{LedgerStoreFactory, WriteStrategy, WriterType};

    fn test_store_factory() -> Arc<dyn LedgerStoreFactory> {
        default_ledger_store_factory()
    }

    fn new_ledger() -> Ledger {
        Ledger::new_null(test_store_factory())
    }

    fn new_builder() -> NullLedgerBuilder {
        Ledger::new_null_builder(test_store_factory())
    }

    #[test]
    fn insert_open_state_block() {
        let (mut block, instructions) = open_state_block_instructions();
        let ledger = new_ledger();

        let result = insert(&ledger, &mut block, &instructions);

        let expected_block = SavedBlock::new(block.clone(), instructions.set_sideband.clone());
        assert_eq!(result.saved_blocks, vec![expected_block]);
        assert_eq!(
            result.saved_accounts,
            vec![(instructions.account, instructions.set_account_info.clone())]
        );
        assert_eq!(
            ledger
                .rep_weights
                .weight(&instructions.set_account_info.representative),
            instructions.set_account_info.balance
        );
        assert_eq!(ledger.store.cache().block_count.load(Ordering::Relaxed), 2);
        assert_eq!(result.deleted_pending, Vec::new());
    }

    #[test]
    fn delete_old_pending() {
        let (mut block, mut instructions) = legacy_open_block_instructions();
        let pending_key = PendingKey::new_test_instance();
        instructions.delete_pending = Some(pending_key.clone());
        let ledger = new_ledger();

        let result = insert(&ledger, &mut block, &instructions);

        assert_eq!(result.deleted_pending, vec![pending_key]);
    }

    #[test]
    fn insert_pending() {
        let (mut block, mut instructions) = legacy_open_block_instructions();
        let pending_key = PendingKey::new_test_instance();
        let pending_info = PendingInfo::new_test_instance();
        instructions.insert_pending = Some((pending_key.clone(), pending_info.clone()));
        let ledger = new_ledger();

        let result = insert(&ledger, &mut block, &instructions);

        assert_eq!(result.saved_pending, vec![(pending_key, pending_info)]);
    }

    #[test]
    fn update_representative() {
        let old_representative = PublicKey::from(1111);
        let new_representative = PublicKey::from(2222);
        let open = TestBlockBuilder::legacy_open()
            .representative(old_representative)
            .build();
        let sideband = BlockSideband::new_test_instance();
        let open = SavedBlock::new(open, sideband.clone());

        let state = TestBlockBuilder::state()
            .previous(open.hash())
            .representative(new_representative)
            .balance(sideband.balance)
            .build();
        let (mut state, instructions) = state_block_instructions_for(&open, state);

        let ledger = new_builder()
            .block(&open)
            .account_info(
                &open.account(),
                &AccountInfo {
                    head: open.hash(),
                    representative: old_representative,
                    open_block: open.hash(),
                    balance: open.balance(),
                    modified: UnixTimestamp::new(1),
                    block_count: 1,
                    epoch: Epoch::Epoch0,
                },
            )
            .finish();

        insert(&ledger, &mut state, &instructions);

        assert_eq!(
            ledger.rep_weights.weight(&new_representative),
            instructions.set_account_info.balance
        );
    }

    #[test]
    fn no_successor_for_open_block() {
        let (mut block, instructions) = open_state_block_instructions();
        let ledger = new_ledger();

        let result = insert(&ledger, &mut block, &instructions);

        assert_eq!(result.saved_successors, Vec::new());
    }

    #[test]
    fn insert_successor() {
        let open = TestBlockBuilder::legacy_open().build();
        let sideband = BlockSideband::new_test_instance();
        let open = SavedBlock::new(open, sideband.clone());

        let state = TestBlockBuilder::state().previous(open.hash()).build();
        let (mut state, instructions) = state_block_instructions_for(&open, state);

        let ledger = new_builder()
            .block(&open)
            .account_info(
                &open.account(),
                &AccountInfo {
                    head: open.hash(),
                    representative: open.account().into(),
                    open_block: open.hash(),
                    balance: open.balance(),
                    modified: UnixTimestamp::new(1),
                    block_count: 1,
                    epoch: Epoch::Epoch0,
                },
            )
            .finish();

        let result = insert(&ledger, &mut state, &instructions);

        assert_eq!(result.saved_successors, vec![(open.hash(), state.hash())]);
    }

    #[test]
    fn returns_existing_block_when_account_changed() {
        let (mut block, instructions) = legacy_open_block_instructions();
        let ledger = new_ledger();

        // First insertion succeeds and commits
        let mut first_txn = ledger.begin_write_with(WriterType::Testing, WriteStrategy::Optimistic);
        let mut first_deferred = DeferredLedgerOperations::new();
        let (saved, inserted, _) =
            BlockInserter::new(&ledger, first_txn.as_mut(), &mut block, &instructions)
                .insert(&mut first_deferred);
        assert!(inserted);
        let disposition = commit_block_txn(&ledger, first_txn, inserted, saved.as_ref());
        if inserted && matches!(disposition, CommitDisposition::Success) {
            first_deferred.execute(&ledger);
        }

        // Second insertion sees updated account info and should surface the preexisting block
        let mut block_again = block.clone();
        let mut second_txn =
            ledger.begin_write_with(WriterType::Testing, WriteStrategy::Optimistic);
        let mut second_deferred = DeferredLedgerOperations::new();
        let (saved_again, inserted_again, preexisting_again) = BlockInserter::new(
            &ledger,
            second_txn.as_mut(),
            &mut block_again,
            &instructions,
        )
        .insert(&mut second_deferred);

        assert!(!inserted_again);
        assert!(preexisting_again);
        assert!(saved_again.is_some());
        let disposition =
            commit_block_txn(&ledger, second_txn, inserted_again, saved_again.as_ref());
        if inserted_again && matches!(disposition, CommitDisposition::Success) {
            second_deferred.execute(&ledger);
        }

        assert_eq!(ledger.store.cache().block_count.load(Ordering::SeqCst), 2);
    }

    fn insert(
        ledger: &Ledger,
        block: &mut Block,
        instructions: &BlockInsertInstructions,
    ) -> InsertResult {
        let mut txn = ledger.begin_write_with(WriterType::Testing, WriteStrategy::Optimistic);
        let saved_blocks = ledger.store.block().track_puts();
        let saved_accounts = ledger.store.account().track_puts();
        let saved_pending = ledger.store.pending().track_puts();
        let saved_successors = ledger.store.successors().track_puts();
        let deleted_pending = ledger.store.pending().track_deletions();
        let mut deferred = DeferredLedgerOperations::new();

        let mut block_inserter = BlockInserter::new(&ledger, txn.as_mut(), block, &instructions);
        let (saved_block, inserted, _) = block_inserter.insert(&mut deferred);
        assert!(inserted, "expected block to be inserted");
        saved_block.as_ref().expect("block should be saved");
        let disposition = commit_block_txn(ledger, txn, inserted, saved_block.as_ref());
        if inserted && matches!(disposition, CommitDisposition::Success) {
            deferred.execute(ledger);
        }

        InsertResult {
            saved_blocks: saved_blocks.output(),
            saved_accounts: saved_accounts.output(),
            saved_pending: saved_pending.output(),
            saved_successors: saved_successors.output(),
            deleted_pending: deleted_pending.output(),
        }
    }

    struct InsertResult {
        saved_blocks: Vec<SavedBlock>,
        saved_accounts: Vec<(Account, AccountInfo)>,
        saved_pending: Vec<(PendingKey, PendingInfo)>,
        saved_successors: Vec<(BlockHash, BlockHash)>,
        deleted_pending: Vec<PendingKey>,
    }

    #[test]
    fn duplicate_insert_does_not_increment_cache() {
        let (mut block, instructions) = legacy_open_block_instructions();
        let ledger = new_ledger();

        let start_inserts = ledger.block_cache_inserts();
        let start_duplicates = ledger.block_cache_duplicate_inserts();

        {
            let mut txn = ledger.begin_write_with(WriterType::Testing, WriteStrategy::Optimistic);
            let mut deferred = DeferredLedgerOperations::new();
            let (saved_block, inserted, preexisting) =
                BlockInserter::new(&ledger, txn.as_mut(), &mut block, &instructions)
                    .insert(&mut deferred);
            assert!(inserted);
            assert!(!preexisting);
            assert!(saved_block.is_some());
            let disposition = commit_block_txn(&ledger, txn, inserted, saved_block.as_ref());
            if inserted && matches!(disposition, CommitDisposition::Success) {
                deferred.execute(&ledger);
            }
        }

        assert_eq!(ledger.block_cache_inserts(), start_inserts + 1);
        assert_eq!(ledger.block_cache_duplicate_inserts(), start_duplicates);

        let read_txn = ledger.store.begin_read();
        let current_info = ledger
            .store
            .account()
            .get(read_txn.as_ref(), &instructions.account)
            .unwrap();
        drop(read_txn);

        let duplicate_instructions = BlockInsertInstructions {
            account: instructions.account,
            old_account_info: current_info.clone(),
            set_account_info: current_info,
            delete_pending: None,
            insert_pending: None,
            set_sideband: instructions.set_sideband.clone(),
            is_epoch_block: instructions.is_epoch_block,
        };

        {
            let mut txn = ledger.begin_write_with(WriterType::Testing, WriteStrategy::Optimistic);
            let mut deferred = DeferredLedgerOperations::new();
            let (_saved_block, inserted, preexisting) =
                BlockInserter::new(&ledger, txn.as_mut(), &mut block, &duplicate_instructions)
                    .insert(&mut deferred);
            assert!(!inserted, "duplicate insert should not report insertion");
            assert!(preexisting);
            let disposition = commit_block_txn(&ledger, txn, inserted, None);
            if inserted && matches!(disposition, CommitDisposition::Success) {
                deferred.execute(&ledger);
            }
        }

        assert_eq!(ledger.block_cache_inserts(), start_inserts + 1);
        assert_eq!(ledger.block_cache_duplicate_inserts(), start_duplicates + 1);
    }

    #[test]
    fn duplicate_insert_across_txns_increments_cache() {
        let (mut block, instructions) = legacy_open_block_instructions();
        let ledger = new_ledger();

        let start_inserts = ledger.block_cache_inserts();

        {
            let mut txn = ledger.begin_write_with(WriterType::Testing, WriteStrategy::Optimistic);
            let mut deferred = DeferredLedgerOperations::new();

            let (saved_block, inserted, preexisting) =
                BlockInserter::new(&ledger, txn.as_mut(), &mut block, &instructions)
                    .insert(&mut deferred);
            assert!(inserted);
            assert!(!preexisting);
            assert!(saved_block.is_some());

            let mut txn2 = ledger.begin_write_with(WriterType::Testing, WriteStrategy::Optimistic);
            let mut deferred2 = DeferredLedgerOperations::new();
            let (duplicate_block, inserted_again, preexisting_again) =
                BlockInserter::new(&ledger, txn2.as_mut(), &mut block, &instructions)
                    .insert(&mut deferred2);
            assert!(inserted_again);
            assert!(!preexisting_again);

            let disposition = commit_block_txn(&ledger, txn, inserted, saved_block.as_ref());
            let disposition2 =
                commit_block_txn(&ledger, txn2, inserted_again, duplicate_block.as_ref());

            if inserted && matches!(disposition, CommitDisposition::Success) {
                deferred.execute(&ledger);
            }
            if inserted_again && matches!(disposition2, CommitDisposition::Success) {
                deferred2.execute(&ledger);
            }
        }

        assert_eq!(
            ledger.block_cache_inserts(),
            start_inserts + 1,
            "duplicate insert across txns increments cache twice"
        );
    }

    #[test]
    fn parallel_duplicate_inserts_diverge_cache() {
        let (block, instructions) = legacy_open_block_instructions();
        let ledger = Arc::new(new_ledger());
        let block = Arc::new(block);
        let instructions = Arc::new(instructions);

        let mut handles = Vec::new();
        for _ in 0..100 {
            let ledger = Arc::clone(&ledger);
            let block = Arc::clone(&block);
            let instructions = Arc::clone(&instructions);
            handles.push(std::thread::spawn(move || {
                let mut block_clone = (*block).clone();
                let instructions_clone = (*instructions).clone();
                let mut txn =
                    ledger.begin_write_with(WriterType::Testing, WriteStrategy::Optimistic);
                let mut deferred = DeferredLedgerOperations::new();
                let (saved_block, inserted, _) = BlockInserter::new(
                    &ledger,
                    txn.as_mut(),
                    &mut block_clone,
                    &instructions_clone,
                )
                .insert(&mut deferred);
                let disposition = commit_block_txn(&ledger, txn, inserted, saved_block.as_ref());
                if matches!(disposition, CommitDisposition::Success) && inserted {
                    deferred.execute(&ledger);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let tx = ledger.store.begin_read();
        let store_count = ledger.store.block().iter(tx.as_ref()).count() as u64;
        assert_eq!(
            ledger.block_count(),
            store_count,
            "parallel duplicate inserts diverge cache from store"
        );
    }

    fn open_state_block_instructions() -> (Block, BlockInsertInstructions) {
        let block = TestBlockBuilder::state().previous(BlockHash::ZERO).build();
        let sideband = BlockSideband::new_test_instance();
        let account_info = AccountInfo {
            head: block.hash(),
            open_block: block.hash(),
            ..AccountInfo::new_test_instance()
        };
        let instructions = BlockInsertInstructions {
            account: Account::from(1),
            old_account_info: AccountInfo::default(),
            set_account_info: account_info,
            delete_pending: None,
            insert_pending: None,
            set_sideband: sideband,
            is_epoch_block: false,
        };

        (block, instructions)
    }

    fn state_block_instructions_for(
        previous: &SavedBlock,
        block: Block,
    ) -> (Block, BlockInsertInstructions) {
        let sideband = BlockSideband {
            balance: block.balance_field().unwrap(),
            account: block.account_field().unwrap(),
            ..BlockSideband::new_test_instance()
        };
        let old_account_info = AccountInfo {
            head: previous.hash(),
            balance: previous.balance(),
            ..AccountInfo::new_test_instance()
        };
        let new_account_info = AccountInfo {
            head: block.hash(),
            open_block: block.hash(),
            balance: block.balance_field().unwrap(),
            representative: block.representative_field().unwrap(),
            ..AccountInfo::new_test_instance()
        };
        let instructions = BlockInsertInstructions {
            account: previous.account(),
            old_account_info,
            set_account_info: new_account_info,
            delete_pending: None,
            insert_pending: None,
            set_sideband: sideband,
            is_epoch_block: false,
        };

        (block, instructions)
    }
}
