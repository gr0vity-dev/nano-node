use rsnano_types::{Block, BlockHash};

use crate::LedgerStore;
use store_traits::LedgerReadTxn;

/// Goes back in the block history until it finds a block with representative information
pub(crate) struct RepresentativeBlockFinder<'a> {
    txn: &'a dyn LedgerReadTxn,
    store: &'a dyn LedgerStore,
}

impl<'a> RepresentativeBlockFinder<'a> {
    pub fn new(txn: &'a dyn LedgerReadTxn, store: &'a dyn LedgerStore) -> Self {
        Self { txn, store }
    }

    pub fn find_rep_block(&self, hash: BlockHash) -> BlockHash {
        let mut current = hash;
        let mut result = BlockHash::ZERO;
        while result.is_zero() {
            let Some(block) = self.store.block().get(self.txn, &current) else {
                return BlockHash::ZERO;
            };
            (current, result) = match &*block {
                Block::LegacySend(_) => (block.previous(), BlockHash::ZERO),
                Block::LegacyReceive(_) => (block.previous(), BlockHash::ZERO),
                Block::LegacyOpen(_) => (BlockHash::ZERO, block.hash()),
                Block::LegacyChange(_) => (BlockHash::ZERO, block.hash()),
                Block::State(_) => (BlockHash::ZERO, block.hash()),
            };
        }

        result
    }
}
