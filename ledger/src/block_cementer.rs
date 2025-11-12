use std::{collections::VecDeque, sync::atomic::Ordering};

use rsnano_types::{BlockHash, ConfirmationHeightInfo, SavedBlock};
use rsnano_utils::stats::{DetailType, Direction, StatType, Stats};

use rsnano_nullable_lmdb::Transaction;

use crate::{LedgerConstants, LedgerStore, LedgerWriteTransaction, refresh_write_txn};

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
        mut txn: LedgerWriteTransaction,
        target_hash: BlockHash,
        max_blocks: usize,
    ) -> (LedgerWriteTransaction, Vec<SavedBlock>) {
        let mut result = Vec::new();

        let mut stack = VecDeque::new();
        stack.push_back(target_hash);
        while let Some(&hash) = stack.back() {
            let block = self.store.block().get(&*txn, &hash).unwrap();

            let dependents =
                block.dependent_blocks(&self.constants.epochs, &self.constants.genesis_account);
            for dependent in dependents.iter() {
                if !dependent.is_zero() && !self.is_confirmed(&txn, dependent) {
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
                if !self.is_confirmed(&txn, &hash) {
                    // We must only confirm blocks that have their dependencies confirmed

                    let conf_height = ConfirmationHeightInfo::new(block.height(), block.hash());

                    // Update store
                    self.store
                        .confirmation_height()
                        .put(&mut txn, &block.account(), &conf_height);
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
            } else {
                // Unconfirmed dependencies were added
            }

            // Refresh the transaction to avoid long-running transactions
            // Ensure that the block wasn't rolled back during the refresh

            if txn.is_refresh_needed() {
                txn = refresh_write_txn(self.store, txn);
                if !self.store.block().exists(&*txn, &target_hash) {
                    break; // Block was rolled back during cementing
                }
            }

            // Early return might leave parts of the dependency tree unconfirmed
            if result.len() >= max_blocks {
                break;
            }
        }
        (txn, result)
    }

    fn is_confirmed(&self, tx: &LedgerWriteTransaction, hash: &BlockHash) -> bool {
        let Some(block) = self.store.block().get(&*tx, hash) else {
            return false;
        };
        let Some(info) = self.store.confirmation_height().get(tx, &block.account()) else {
            return false;
        };

        block.height() <= info.height
    }
}
