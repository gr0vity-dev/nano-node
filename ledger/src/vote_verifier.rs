use std::collections::VecDeque;

use rsnano_types::{BlockHash, Root};

use crate::{AnySet, BorrowingAnySet, LedgerConstants, LedgerStore, OwningAnySet};
use store_traits::LedgerWriteTxn;

/// Verifies whether a vote (or a final vote) can be generated for a given block
pub(crate) struct VoteVerifier<'a> {
    pub constants: &'a LedgerConstants,
    pub store: &'a dyn LedgerStore,
}

impl<'a> VoteVerifier<'a> {
    pub fn verify_votes(
        &self,
        candidates: VecDeque<(Root, BlockHash)>,
        is_final: bool,
    ) -> VecDeque<(Root, BlockHash)> {
        let mut verified = VecDeque::new();

        if is_final {
            let mut txn = self.store.begin_write();
            for (root, hash) in &candidates {
                if txn.is_refresh_needed() {
                    txn.commit();
                    txn = self.store.begin_write();
                }
                if self.should_vote_final(txn.as_mut(), root, hash) {
                    verified.push_back((*root, *hash));
                }
            }
            txn.commit();
        } else {
            let mut any = OwningAnySet::new(self.store, self.constants);
            for (root, hash) in &candidates {
                if any.is_refresh_needed() {
                    any = OwningAnySet::new(self.store, self.constants);
                }
                if self.should_vote_non_final(&any, root, hash) {
                    verified.push_back((*root, *hash));
                }
            }
        };

        verified
    }

    fn should_vote_non_final(&self, any: &impl AnySet, root: &Root, hash: &BlockHash) -> bool {
        let Some(block) = any.get_block(hash) else {
            return false;
        };
        debug_assert!(block.root() == *root);
        any.dependents_confirmed(&block)
    }

    fn should_vote_final(
        &self,
        tx: &mut dyn LedgerWriteTxn,
        root: &Root,
        hash: &BlockHash,
    ) -> bool {
        let any = BorrowingAnySet {
            constants: &self.constants,
            store: self.store,
            tx,
        };
        let Some(block) = any.get_block(hash) else {
            return false;
        };
        debug_assert!(block.root() == *root);
        any.dependents_confirmed(&block)
            && self
                .store
                .final_vote()
                .put(tx, &block.qualified_root(), hash)
    }
}
