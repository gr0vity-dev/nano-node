use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use crate::ledger_factory::default_ledger_store_factory;
use rsnano_ledger::{Ledger, LedgerSet};
use rsnano_network::ChannelId;
use rsnano_nullable_clock::SteadyClock;
use rsnano_types::{Block, BlockHash};
use rsnano_utils::{
    CancellationToken,
    stats::{StatsCollection, StatsSource},
    ticker::Tickable,
};

use super::UncheckedMap;
use crate::block_processing::{BlockContext, BlockProcessorQueue, BlockSource};
use tracing::trace;

/// Re-enqueues an unchecked block when its missing dependency block got inserted into the ledger
#[derive(Clone)]
pub struct UncheckedBlockReenqueuer {
    unchecked: Arc<Mutex<UncheckedMap>>,
    ledger: Arc<Ledger>,
    process_queue: Arc<BlockProcessorQueue>,
    stats: Arc<UncheckedBlockReenqueuerStats>,
    clock: Arc<SteadyClock>,
    candidates: Vec<BlockHash>,
    satisfied_blocks: Vec<Block>,
    batch_size: usize,
    next_search_start: BlockHash,
}

impl UncheckedBlockReenqueuer {
    pub fn new(
        unchecked: Arc<Mutex<UncheckedMap>>,
        ledger: Arc<Ledger>,
        process_queue: Arc<BlockProcessorQueue>,
        clock: Arc<SteadyClock>,
    ) -> Self {
        Self {
            unchecked,
            ledger,
            process_queue,
            stats: Arc::new(UncheckedBlockReenqueuerStats::default()),
            clock,
            satisfied_blocks: Vec::new(),
            candidates: Vec::new(),
            batch_size: 1000,
            next_search_start: BlockHash::ZERO,
        }
    }

    pub fn new_null() -> Self {
        Self::new(
            Arc::new(Mutex::new(UncheckedMap::default())),
            Arc::new(Ledger::new_null(default_ledger_store_factory())),
            Arc::new(BlockProcessorQueue::new_null()),
            Arc::new(SteadyClock::new_null()),
        )
    }

    pub fn set_batch_size(&mut self, batch_size: usize) {
        self.batch_size = batch_size;
    }

    pub(crate) fn stats(&self) -> &Arc<UncheckedBlockReenqueuerStats> {
        &self.stats
    }

    pub fn enqueue_blocks_with_dependency(&mut self, dependency_hash: BlockHash) {
        self.find_satisfied_blocks_for(dependency_hash);
        self.enqueue_satisfied_blocks();
    }

    fn find_satisfied_blocks_for(&mut self, dependency_hash: BlockHash) {
        let mut unchecked = self.unchecked.lock().unwrap();
        unchecked.pop_dependend_blocks(dependency_hash, &mut self.satisfied_blocks);
    }

    fn find_satisfied_blocks(&mut self) {
        self.fill_candidates();
        self.filter_satisfied_candidates();
        self.pop_satisfied_candidates();
    }

    fn fill_candidates(&mut self) {
        self.candidates.clear();
        let unchecked = self.unchecked.lock().unwrap();

        self.candidates.extend(
            unchecked
                .iter_start(self.next_search_start)
                .map(|(dependency_hash, _)| *dependency_hash)
                .take(self.batch_size),
        );

        self.next_search_start = self
            .candidates
            .last()
            .and_then(|i| i.inc())
            .unwrap_or(BlockHash::ZERO)
    }

    fn filter_satisfied_candidates(&mut self) {
        let any = self.ledger.any();
        self.candidates.retain(|h| any.block_exists(h));
    }

    fn pop_satisfied_candidates(&mut self) {
        let mut unchecked = self.unchecked.lock().unwrap();
        for dependency_hash in self.candidates.drain(..) {
            unchecked.pop_dependend_blocks(dependency_hash, &mut self.satisfied_blocks);
        }
    }

    fn enqueue_satisfied_blocks(&mut self) {
        for block in self.satisfied_blocks.drain(..) {
            self.stats
                .unchecked_satisfied
                .fetch_add(1, Ordering::Relaxed);

            trace!(block_hash = ?block.hash(), "Reenqueue unchecked block");

            self.process_queue.push(BlockContext::new(
                block,
                BlockSource::Unchecked,
                ChannelId::LOOPBACK,
            ));
        }
    }

    fn discard_old_entries(&mut self) {
        let now = self.clock.now();
        let cutoff = now - Duration::from_secs(60 * 15);
        self.unchecked.lock().unwrap().discard_old_entries(cutoff);
    }
}

impl Tickable for UncheckedBlockReenqueuer {
    fn tick(&mut self, _cancel_token: &CancellationToken) {
        self.find_satisfied_blocks();
        self.enqueue_satisfied_blocks();
        self.discard_old_entries();
    }
}

#[derive(Default)]
pub(crate) struct UncheckedBlockReenqueuerStats {
    unchecked_satisfied: AtomicU64,
}

impl StatsSource for UncheckedBlockReenqueuerStats {
    fn collect_stats(&self, result: &mut StatsCollection) {
        result.insert(
            "unchecked",
            "satisfied",
            self.unchecked_satisfied.load(Ordering::Relaxed),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_processing::BlockSource;
    use rsnano_nullable_clock::Timestamp;
    use rsnano_types::{Block, BlockHash, SavedBlock};
    use std::time::Duration;

    #[test]
    fn reenqueue_satisfied_block() {
        let now = Timestamp::new_test_instance();
        let dependency_block = SavedBlock::new_test_instance();
        let unchecked_block = Block::new_test_instance_with_key(2);

        let unchecked = Arc::new(Mutex::new(UncheckedMap::default()));
        unchecked
            .lock()
            .unwrap()
            .put(dependency_block.hash(), unchecked_block.clone(), now);

        let ledger = Arc::new(
            Ledger::new_null_builder(default_ledger_store_factory())
                .block(&dependency_block)
                .finish(),
        );
        let process_queue = Arc::new(BlockProcessorQueue::default());
        let clock = Arc::new(SteadyClock::new_null());
        let mut reenqueuer =
            UncheckedBlockReenqueuer::new(unchecked, ledger, process_queue.clone(), clock);

        reenqueuer.tick(&CancellationToken::new_null());

        assert_eq!(process_queue.total_queue_len(), 1, "enqueued blocks count");
        let enqueued = process_queue.pop_blocking().unwrap().pop_front().unwrap();
        assert_eq!(enqueued.block.hash(), unchecked_block.hash());
        assert_eq!(enqueued.source, BlockSource::Unchecked);
    }

    #[test]
    fn dont_enqueue_if_dependency_still_isnt_satisfied() {
        let now = Timestamp::new_test_instance();
        let dependency_hash = BlockHash::from(123);
        let unchecked_block = Block::new_test_instance_with_key(2);

        let unchecked = Arc::new(Mutex::new(UncheckedMap::default()));
        unchecked
            .lock()
            .unwrap()
            .put(dependency_hash, unchecked_block.clone(), now);

        let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));
        let process_queue = Arc::new(BlockProcessorQueue::default());
        let clock = Arc::new(SteadyClock::new_null());
        let mut reenqueuer =
            UncheckedBlockReenqueuer::new(unchecked, ledger, process_queue.clone(), clock);

        reenqueuer.tick(&CancellationToken::new_null());

        assert_eq!(process_queue.total_queue_len(), 0, "enqueued blocks count");
    }

    #[test]
    fn reenqueue_multiple() {
        let now = Timestamp::new_test_instance();
        let dependency_block1 = SavedBlock::new_test_instance_with_key(1);
        let unchecked_block1 = Block::new_test_instance_with_key(2);

        let dependency_block2 = SavedBlock::new_test_instance_with_key(3);
        let unchecked_block2 = Block::new_test_instance_with_key(4);

        let mut unchecked = UncheckedMap::default();
        unchecked.put(dependency_block1.hash(), unchecked_block1.clone(), now);
        unchecked.put(dependency_block2.hash(), unchecked_block2.clone(), now);
        let unchecked = Arc::new(Mutex::new(unchecked));

        let ledger = Arc::new(
            Ledger::new_null_builder(default_ledger_store_factory())
                .block(&dependency_block1)
                .block(&dependency_block2)
                .finish(),
        );
        let process_queue = Arc::new(BlockProcessorQueue::default());
        let clock = Arc::new(SteadyClock::new_null());
        let mut reenqueuer =
            UncheckedBlockReenqueuer::new(unchecked, ledger, process_queue.clone(), clock);

        reenqueuer.tick(&CancellationToken::new_null());

        assert_eq!(process_queue.total_queue_len(), 2, "enqueued blocks count");
    }

    #[test]
    fn work_in_batches() {
        let now = Timestamp::new_test_instance();
        let dependency_block1 = SavedBlock::new_test_instance_with_key(1);
        let unchecked_block1 = Block::new_test_instance_with_key(2);

        let dependency_block2 = SavedBlock::new_test_instance_with_key(3);
        let unchecked_block2 = Block::new_test_instance_with_key(4);

        let dependency_block3 = SavedBlock::new_test_instance_with_key(5);
        let unchecked_block3 = Block::new_test_instance_with_key(6);

        let mut unchecked = UncheckedMap::default();
        unchecked.put(dependency_block1.hash(), unchecked_block1.clone(), now);
        unchecked.put(dependency_block2.hash(), unchecked_block2.clone(), now);
        unchecked.put(dependency_block3.hash(), unchecked_block3.clone(), now);
        let unchecked = Arc::new(Mutex::new(unchecked));

        let ledger = Arc::new(
            Ledger::new_null_builder(default_ledger_store_factory())
                .block(&dependency_block1)
                .block(&dependency_block2)
                .block(&dependency_block3)
                .finish(),
        );
        let process_queue = Arc::new(BlockProcessorQueue::default());
        let clock = Arc::new(SteadyClock::new_null());
        let mut reenqueuer =
            UncheckedBlockReenqueuer::new(unchecked, ledger, process_queue.clone(), clock);
        reenqueuer.set_batch_size(2);

        reenqueuer.tick(&CancellationToken::new_null());
        assert_eq!(
            process_queue.total_queue_len(),
            2,
            "enqueued blocks count (first batch)"
        );

        reenqueuer.tick(&CancellationToken::new_null());
        assert_eq!(
            process_queue.total_queue_len(),
            3,
            "enqueued blocks count (second batch)"
        );
    }

    #[test]
    fn continue_search_from_last_tick() {
        let now = Timestamp::new_test_instance();
        let dependency_block1 = SavedBlock::new_test_instance_with_key(1);
        let unchecked_block1 = Block::new_test_instance_with_key(2);

        let dependency_block2 = SavedBlock::new_test_instance_with_key(3);
        let unchecked_block2 = Block::new_test_instance_with_key(4);

        let dependency_block3 = SavedBlock::new_test_instance_with_key(5);
        let unchecked_block3 = Block::new_test_instance_with_key(6);

        let mut unchecked = UncheckedMap::default();
        unchecked.put(dependency_block1.hash(), unchecked_block1.clone(), now);
        unchecked.put(dependency_block2.hash(), unchecked_block2.clone(), now);
        unchecked.put(dependency_block3.hash(), unchecked_block3.clone(), now);
        let unchecked = Arc::new(Mutex::new(unchecked));

        let mut blocks = vec![dependency_block1, dependency_block2, dependency_block3];
        blocks.sort_by_key(|b| b.hash());

        let ledger = Arc::new(
            Ledger::new_null_builder(default_ledger_store_factory())
                .block(&blocks[2])
                .finish(),
        );
        let process_queue = Arc::new(BlockProcessorQueue::default());
        let clock = Arc::new(SteadyClock::new_null());
        let mut reenqueuer =
            UncheckedBlockReenqueuer::new(unchecked, ledger, process_queue.clone(), clock);
        reenqueuer.set_batch_size(2);

        reenqueuer.tick(&CancellationToken::new_null());
        assert_eq!(
            process_queue.total_queue_len(),
            0,
            "enqueued blocks count (first batch)"
        );

        reenqueuer.tick(&CancellationToken::new_null());
        assert_eq!(
            process_queue.total_queue_len(),
            1,
            "enqueued blocks count (second batch)"
        );
    }

    #[test]
    fn discard_old_entries() {
        let now = Timestamp::new_test_instance();
        let dependency_block1 = SavedBlock::new_test_instance_with_key(1);
        let unchecked_block1 = Block::new_test_instance_with_key(2);

        let dependency_block2 = SavedBlock::new_test_instance_with_key(3);
        let unchecked_block2 = Block::new_test_instance_with_key(4);

        let dependency_block3 = SavedBlock::new_test_instance_with_key(5);
        let unchecked_block3 = Block::new_test_instance_with_key(6);

        let mut unchecked = UncheckedMap::default();
        unchecked.put(
            dependency_block1.hash(),
            unchecked_block1.clone(),
            now - Duration::from_secs(60 * 15),
        );
        unchecked.put(dependency_block2.hash(), unchecked_block2.clone(), now);
        unchecked.put(dependency_block3.hash(), unchecked_block3.clone(), now);
        let unchecked = Arc::new(Mutex::new(unchecked));

        let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));
        let process_queue = Arc::new(BlockProcessorQueue::default());
        let clock = Arc::new(SteadyClock::new_null());
        let mut reenqueuer =
            UncheckedBlockReenqueuer::new(unchecked.clone(), ledger, process_queue.clone(), clock);

        reenqueuer.tick(&CancellationToken::new_null());

        assert_eq!(unchecked.lock().unwrap().len(), 2);
    }
}
