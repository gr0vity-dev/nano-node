use std::{collections::VecDeque, sync::atomic::Ordering};

use rsnano_core::{BlockHash, ConfirmationHeightInfo, SavedBlock};
use rsnano_nullable_lmdb::{Transaction, WriteTransaction};
use rsnano_stats::{DetailType, Direction, StatType, Stats};
use rsnano_store_lmdb::LmdbStore;

use crate::LedgerConstants;

/// Cements Blocks in the ledger
pub(crate) struct BlockCementer<'a> {
    constants: &'a LedgerConstants,
    store: &'a LmdbStore,
    stats: &'a Stats,
}

impl<'a> BlockCementer<'a> {
    pub(crate) fn new(
        store: &'a LmdbStore,
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
        mut txn: WriteTransaction,
        target_hash: BlockHash,
        max_blocks: usize,
    ) -> (WriteTransaction, Vec<SavedBlock>) {
        let mut result = Vec::new();

        let mut stack = VecDeque::new();
        stack.push_back(target_hash);
        while let Some(&hash) = stack.back() {
            let block = self.store.block.get(&txn, &hash).unwrap();

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
                        .confirmation_height
                        .put(&mut txn, &block.account(), &conf_height);
                    self.store
                        .cache
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
                txn = self.store.env.refresh(txn);
        if {
            use store_api::{BlockStore, StoreProvider};
            let rtx = StoreProvider::begin_read(self.store);
            !<rsnano_store_lmdb::LmdbBlockStore as BlockStore<
                rsnano_store_lmdb::adapter::ReadTxnPub,
                rsnano_store_lmdb::adapter::WriteTxnPub,
            >>::exists(StoreProvider::block(self.store), &rtx, &target_hash)
        } {
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

    fn is_confirmed(&self, tx: &WriteTransaction, hash: &BlockHash) -> bool {
        if self.store.pruned.exists(tx, hash) {
            return true;
        }
        let Some(block) = self.store.block.get(tx, hash) else {
            return false;
        };
        let Some(info) = self.store.confirmation_height.get(tx, &block.account()) else {
            return false;
        };

        block.height() <= info.height
    }
}
