use std::{sync::{Arc, Mutex}, thread::JoinHandle, time::Instant};

use rsnano_ledger::Ledger;
use rsnano_nullable_clock::SteadyClock;
use rsnano_utils::{
    stats::{StatsCollection, StatsSource},
    sync::backpressure_channel::Sender,
};

use super::{
    BlockProcessorQueue, LedgerEvent, UncheckedBlockReenqueuer, UncheckedMap,
    backlog_waiter::BacklogWaiter, block_batch_processor::BlockBatchProcessorStats,
};
use crate::block_processing::block_batch_processor::BlockBatchProcessor;

pub struct BlockProcessor {
    threads: Mutex<Vec<JoinHandle<()>>>,
    process_queue: Arc<BlockProcessorQueue>,
    ledger: Arc<Ledger>,
    unchecked: Arc<Mutex<UncheckedMap>>,
    process_stats: Arc<BlockBatchProcessorStats>,
    backlog_waiter: Arc<BacklogWaiter>,
    event_publisher: Mutex<Option<Sender<LedgerEvent>>>,
    unchecked_reenqueuer: UncheckedBlockReenqueuer,
    clock: Arc<SteadyClock>,
}

impl BlockProcessor {
    pub(crate) fn new(
        process_queue: Arc<BlockProcessorQueue>,
        ledger: Arc<Ledger>,
        unchecked: Arc<Mutex<UncheckedMap>>,
        unchecked_reenqueuer: UncheckedBlockReenqueuer,
        backlog_waiter: Arc<BacklogWaiter>,
        event_publisher: Sender<LedgerEvent>,
        clock: Arc<SteadyClock>,
    ) -> Self {
        Self {
            process_queue,
            ledger,
            unchecked,
            unchecked_reenqueuer,
            process_stats: Arc::new(BlockBatchProcessorStats::default()),
            threads: Mutex::new(Vec::new()),
            backlog_waiter,
            event_publisher: Mutex::new(Some(event_publisher)),
            clock,
        }
    }

    pub fn start(&self, thread_count: usize) {
        debug_assert!(self.threads.lock().unwrap().is_empty());
        for _ in 0..thread_count {
            let mut processor_loop = self.create_loop();

            self.threads.lock().unwrap().push(
                std::thread::Builder::new()
                    .name("Blck processing".to_string())
                    .spawn(move || {
                        processor_loop.run();
                    })
                    .unwrap(),
            );
        }
    }

    fn create_loop(&self) -> BlockProcessorLoop {
        BlockProcessorLoop {
            queue: self.process_queue.clone(),
            process: self.create_block_batch_processor(),
            backlog_waiter: self.backlog_waiter.clone(),
        }
    }

    fn create_block_batch_processor(&self) -> BlockBatchProcessor {
        BlockBatchProcessor {
            ledger: self.ledger.clone(),
            unchecked: self.unchecked.clone(),
            stats: self.process_stats.clone(),
            event_publisher: self
                .event_publisher
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .clone(),
            unchecked_reenqueuer: self.unchecked_reenqueuer.clone(),
            clock: self.clock.clone(),
        }
    }

    pub fn stop(&self) {
        drop(self.event_publisher.lock().unwrap().take());
        self.process_queue.stop();
        let mut threads = self.threads.lock().unwrap();
        for join_handle in threads.drain(..) {
            join_handle.join().unwrap();
        }
    }

    pub fn stats(&self) -> Arc<BlockBatchProcessorStats> {
        Arc::clone(&self.process_stats)
    }
}

impl Drop for BlockProcessor {
    fn drop(&mut self) {
        self.stop();
    }
}

impl StatsSource for BlockProcessor {
    fn collect_stats(&self, result: &mut StatsCollection) {
        self.process_stats.collect_stats(result);

        // Expose queue depth for visibility
        result.insert(
            "block_processor_queue",
            "size",
            self.process_queue.total_queue_len() as u64,
        );
    }
}

struct BlockProcessorLoop {
    queue: Arc<BlockProcessorQueue>,
    process: BlockBatchProcessor,
    backlog_waiter: Arc<BacklogWaiter>,
}

impl BlockProcessorLoop {
    fn run(&mut self) {
        while let Some(blocks) = {
            let wait_start = Instant::now();
            let batch = self.queue.pop_blocking();
            batch.map(|b| (b, wait_start.elapsed().as_nanos() as u64))
        } {
            let (blocks, dequeue_wait_ns) = blocks;
            self.backlog_waiter.wait_for_backlog();

            if self.queue.stopped() {
                break;
            }

            self.process.process_blocks(blocks, dequeue_wait_ns);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_processing::BlockContext;
    use crate::block_processing::{BlockSource, ProcessQueueConfig};
    use crate::ledger_factory::default_ledger_store_factory;
    use rsnano_ledger::LedgerSet;
    use rsnano_ledger::test_helpers::SavedBlockLatticeBuilder;
    use rsnano_network::ChannelId;
    use rsnano_types::{Amount, Block, PrivateKey, WorkNonce};
    use rsnano_utils::sync::backpressure_channel::channel;

    #[test]
    fn wait_for_backlog() {
        let queue = Arc::new(BlockProcessorQueue::new_null_with(vec![
            BlockContext::new_test_instance().into(),
        ]));
        let process = BlockBatchProcessor::new_null();
        let backlog_waiter = Arc::new(BacklogWaiter::new_null());

        let mut processor_loop = BlockProcessorLoop {
            queue,
            process,
            backlog_waiter: backlog_waiter.clone(),
        };

        processor_loop.run();

        assert_eq!(backlog_waiter.call_count(), 1);
    }

    #[test]
    fn records_max_concurrent_optimistic_writers() {
        let queue = Arc::new(BlockProcessorQueue::new(ProcessQueueConfig {
            batch_size: 1,
            ..Default::default()
        }));
        let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));
        let unchecked = Arc::new(Mutex::new(UncheckedMap::default()));
        let clock = Arc::new(SteadyClock::new_null());
        let reenqueuer = UncheckedBlockReenqueuer::new(
            unchecked.clone(),
            ledger.clone(),
            queue.clone(),
            clock.clone(),
        );
        let backlog_waiter = Arc::new(BacklogWaiter::new(
            queue.clone(),
            ledger.clone(),
            clock.clone(),
            10_000,
        ));
        let (event_publisher, _event_receiver) = channel(0);
        let processor = BlockProcessor::new(
            queue.clone(),
            ledger.clone(),
            unchecked.clone(),
            reenqueuer,
            backlog_waiter,
            event_publisher,
            clock.clone(),
        );
        processor.start(2);

        let mut lattice = SavedBlockLatticeBuilder::new();
        let key1 = PrivateKey::from(1);
        let key2 = PrivateKey::from(2);
        let send1 = lattice.genesis().send(&key1, Amount::nano(2));
        let send2 = lattice.genesis().send(&key2, Amount::nano(2));
        let open1 = lattice.account(&key1).receive(&send1);
        let open2 = lattice.account(&key2).receive(&send2);
        for block in [&send1, &open1, &send2, &open2] {
            let mut block: Block = block.clone().into();
            block.set_work(WorkNonce::new(u64::MAX));
            ledger.process_one(&block).unwrap();
        }
        let prefunded = ledger.block_count();

        let mut blocks = Vec::new();
        for i in 0..16 {
            let block = if i % 2 == 0 {
                lattice.account(&key1).send(&key2, Amount::raw(1))
            } else {
                lattice.account(&key2).send(&key1, Amount::raw(1))
            };
            let mut block: Block = block.into();
            block.set_work(WorkNonce::new(u64::MAX));
            blocks.push(block);
        }

        for block in &blocks {
            assert!(queue.push(Arc::new(BlockContext::new(
                block.clone(),
                BlockSource::Local,
                ChannelId::LOOPBACK,
            ))));
        }

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if blocks.iter().all(|b| ledger.any().block_exists(&b.hash())) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "blocks were not processed in time"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        processor.stop();

        let stats = processor.stats();
        assert!(
            stats.max_optimistic_concurrency() >= 1,
            "expected at least one optimistic writer"
        );
        assert_eq!(stats.optimistic_conflicts(), 0);
        assert_eq!(stats.pessimistic_fallbacks(), 0);
        assert_eq!(ledger.block_count(), prefunded + blocks.len() as u64);
    }
}
