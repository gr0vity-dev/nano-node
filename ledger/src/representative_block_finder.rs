use rsnano_core::{Block, BlockHash};
use store_api::{BlockStore, StoreProvider};

/// Goes back in the block history until it finds a block with representative information
pub(crate) struct RepresentativeBlockFinder<'a> {
    store: &'a rsnano_store_lmdb::LmdbStore,
}

impl<'a> RepresentativeBlockFinder<'a> {
    pub fn new(store: &'a rsnano_store_lmdb::LmdbStore) -> Self {
        Self { store }
    }

    pub fn find_rep_block(&self, hash: BlockHash) -> BlockHash {
        let mut current = hash;
        let mut result = BlockHash::zero();
        while result.is_zero() {
            let r = StoreProvider::begin_read(self.store);
            let Some(block) = <rsnano_store_lmdb::LmdbBlockStore as BlockStore<
                rsnano_store_lmdb::adapter::ReadTxnPub,
                rsnano_store_lmdb::adapter::WriteTxnPub,
            >>::get(StoreProvider::block(self.store), &r, &current) else {
                return BlockHash::zero();
            };
            (current, result) = match &*block {
                Block::LegacySend(_) => (block.previous(), BlockHash::zero()),
                Block::LegacyReceive(_) => (block.previous(), BlockHash::zero()),
                Block::LegacyOpen(_) => (BlockHash::zero(), block.hash()),
                Block::LegacyChange(_) => (BlockHash::zero(), block.hash()),
                Block::State(_) => (BlockHash::zero(), block.hash()),
            };
        }

        result
    }
}
