use std::{collections::VecDeque, sync::atomic::Ordering};

use rsnano_types::{BlockHash, ConfirmationHeightInfo, SavedBlock};
use rsnano_utils::stats::{DetailType, Direction, StatType, Stats};

use crate::{LedgerConstants, LedgerStore};
use store_traits::LedgerWriteTxn;

/// Cements Blocks in the ledger
pub(crate) struct BlockCementer<'a> {
    constants: &'a LedgerConstants,
    store: &'a dyn LedgerStore,
    stats: &'a Stats,
}

impl<'a> BlockCementer<'a> {
    pub(crate) fn new(
        store: &'a dyn LedgerStore,
        constants: &'a LedgerConstants,
        stats: &'a Stats,
    ) -> Self {
        Self {
            store,
            constants,
            stats,
        }
    }

    pub(crate) fn confirm(
        &self,
        mut txn: Box<dyn LedgerWriteTxn>,
        target_hash: BlockHash,
        max_blocks: usize,
    ) -> (Box<dyn LedgerWriteTxn>, Vec<SavedBlock>) {
        let mut result = self.confirm_with_txn(txn.as_mut(), target_hash, max_blocks);
        if txn.is_refresh_needed() {
            txn.commit()
                .unwrap_or_else(|e| panic!("failed to refresh cementing txn: {e}"));
            txn = self.store.begin_write();
            if !self.store.block().exists(txn.as_ref(), &target_hash) {
                return (txn, result);
            }
            result.extend(self.confirm_with_txn(txn.as_mut(), target_hash, max_blocks));
        }
        (txn, result)
    }

    pub(crate) fn confirm_with_txn(
        &self,
        txn: &mut dyn LedgerWriteTxn,
        target_hash: BlockHash,
        max_blocks: usize,
    ) -> Vec<SavedBlock> {
        let mut result = Vec::new();

        let mut stack = VecDeque::new();
        stack.push_back(target_hash);
        while let Some(&hash) = stack.back() {
            let block = self.store.block().get(txn, &hash).unwrap();

            let dependents =
                block.dependent_blocks(&self.constants.epochs, &self.constants.genesis_account);
            for dependent in dependents.iter() {
                if !dependent.is_zero() && !self.is_confirmed(txn, dependent) {
                    self.stats.inc(
                        StatType::ConfirmationHeight,
                        DetailType::DependentUnconfirmed,
                    );

                    stack.push_back(*dependent);

                    // Limit the stack size to avoid excessive memory usage
                    // This will forget the bottom of the dependency tree
                    if stack.len() > max_blocks {
                        stack.pop_front();
                    }
                }
            }

            if stack.back() == Some(&hash) {
                stack.pop_back();
                if !self.is_confirmed(txn, &hash) {
                    let conf_height = ConfirmationHeightInfo::new(block.height(), block.hash());

                    self.store
                        .confirmation_height()
                        .put(txn, &block.account(), &conf_height);
                    self.store
                        .cache()
                        .confirmed_count
                        .fetch_add(1, Ordering::SeqCst);

                    self.stats.add_dir(
                        StatType::ConfirmationHeight,
                        DetailType::BlocksConfirmed,
                        Direction::In,
                        1,
                    );

                    result.push(block);
                }
            }

            if txn.is_refresh_needed() || result.len() >= max_blocks {
                break;
            }
        }
        result
    }

    fn is_confirmed(&self, tx: &dyn LedgerWriteTxn, hash: &BlockHash) -> bool {
        let Some(block) = self.store.block().get(tx, hash) else {
            return false;
        };
        let Some(info) = self.store.confirmation_height().get(tx, &block.account()) else {
            return false;
        };

        block.height() <= info.height
    }
}
