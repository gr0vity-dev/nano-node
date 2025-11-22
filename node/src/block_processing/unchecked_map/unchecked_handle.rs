use std::sync::{Arc, Mutex};

use rsnano_nullable_clock::Timestamp;
use rsnano_types::{Block, BlockHash};

use super::UncheckedMap;

#[derive(Clone)]
pub struct UncheckedHandle {
    map: Arc<Mutex<UncheckedMap>>,
}

impl UncheckedHandle {
    pub(crate) fn new(map: Arc<Mutex<UncheckedMap>>) -> Self {
        Self { map }
    }

    pub fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.lock().unwrap().is_empty()
    }

    pub fn clear(&self) {
        self.map.lock().unwrap().clear();
    }

    pub fn contains_dependency(&self, dependency_hash: BlockHash) -> bool {
        self.map.lock().unwrap().contains_dependency(dependency_hash)
    }

    pub fn submit(&self, dependency: BlockHash, block: Block, now: Timestamp) {
        self.map.lock().unwrap().put(dependency, block, now);
    }

    pub fn all_blocks(&self) -> Vec<(BlockHash, Block)> {
        self.map
            .lock()
            .unwrap()
            .iter()
            .map(|(dep, block)| (*dep, block.clone()))
            .collect()
    }

    pub fn dependent_block_count(&self, dependency: BlockHash) -> usize {
        self.map
            .lock()
            .unwrap()
            .blocks_dependend_on(dependency)
            .count()
    }

    pub fn blocks_starting_at(&self, start_dependency: BlockHash, limit: usize) -> Vec<(BlockHash, Block)> {
        self.map
            .lock()
            .unwrap()
            .iter_start(start_dependency)
            .map(|(dep, block)| (*dep, block.clone()))
            .take(limit)
            .collect()
    }

    pub fn find(&self, hash: &BlockHash) -> Option<Block> {
        self.map
            .lock()
            .unwrap()
            .iter()
            .find_map(|(_, block)| if block.hash() == *hash { Some(block.clone()) } else { None })
    }
}
