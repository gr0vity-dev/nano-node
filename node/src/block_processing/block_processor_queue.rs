use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use strum::{EnumCount, IntoEnumIterator};
use tracing::trace;

use rsnano_ledger::BlockError;
use rsnano_network::{ChannelId, DeadChannelCleanupStep};
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::{Block, SavedBlock};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    fair_queue::FairQueueInfo,
    stats::{StatsCollection, StatsSource},
};

use super::{
    BlockContext, BlockSource,
    process_queue::{ProcessQueue, ProcessQueueConfig},
};

pub struct BlockProcessorQueue {
    queue: Mutex<BlockProcessorQueueImpl>,
    condition: Condvar,
    is_nulled: bool,
    wait_listener: OutputListenerMt<Duration>,
}

impl BlockProcessorQueue {
    pub fn new(config: ProcessQueueConfig) -> Self {
        Self {
            queue: Mutex::new(BlockProcessorQueueImpl::new(config)),
            condition: Condvar::new(),
            is_nulled: false,
            wait_listener: OutputListenerMt::new(),
        }
    }

    pub fn new_null() -> Self {
        Self::new_null_with(Vec::new())
    }

    pub fn new_null_with(blocks: Vec<Arc<BlockContext>>) -> Self {
        let mut queue_impl = BlockProcessorQueueImpl::new(Default::default());
        for ctx in blocks {
            queue_impl.push(ctx);
        }
        if queue_impl.process_queue.is_empty() {
            queue_impl.stopped = true;
        }
        Self {
            queue: Mutex::new(queue_impl),
            condition: Condvar::new(),
            is_nulled: true,
            wait_listener: OutputListenerMt::new(),
        }
    }

    pub fn track_waits(&self) -> Arc<OutputTrackerMt<Duration>> {
        self.wait_listener.track()
    }

    pub fn stop(&self) {
        {
            let mut queue = self.queue.lock().unwrap();
            queue.stopped = true;
        }
        self.condition.notify_all();
    }

    pub fn stopped(&self) -> bool {
        self.queue.lock().unwrap().stopped
    }

    pub fn set_cooldown(&self, cool_down: bool) {
        self.queue.lock().unwrap().cool_down = cool_down;
        self.condition.notify_all();
    }

    pub fn is_cooling_down(&self) -> bool {
        self.queue.lock().unwrap().cool_down
    }

    pub fn wait(&self, duration: Duration) {
        self.wait_listener.emit(duration);

        if self.is_nulled {
            return;
        }

        let guard = self.queue.lock().unwrap();
        let _ = self
            .condition
            .wait_timeout_while(guard, duration, |i| !i.stopped);
    }

    pub(crate) fn pop_blocking(&self) -> Option<VecDeque<Arc<BlockContext>>> {
        let mut queue = self.queue.lock().unwrap();

        loop {
            if queue.stopped {
                return None;
            }

            if !queue.cool_down {
                let batch = queue.process_queue.next_batch();
                if !batch.is_empty() {
                    return Some(batch);
                }
                if self.is_nulled {
                    queue.stopped = true;
                    return None;
                }
            }

            queue = self
                .condition
                .wait_while(queue, |i| i.should_wait())
                .unwrap();
        }
    }

    // TODO: Remove and replace all checks with calls to size (block_source)
    pub fn total_queue_len(&self) -> usize {
        self.queue.lock().unwrap().process_queue.len()
    }

    pub fn queue_len(&self, source: BlockSource) -> usize {
        self.queue.lock().unwrap().process_queue.source_len(source)
    }

    pub fn push(&self, context: impl Into<Arc<BlockContext>>) -> bool {
        let context = context.into();
        let hash = context.block.hash();
        let source = context.source;
        let channel_id = context.channel_id;
        let added = self.queue.lock().unwrap().push(context);

        if added {
            trace!(block_hash = %hash, ?source, ?channel_id, "Enqueued block");
            self.condition.notify_one();
        } else {
            trace!(block_hash = %hash, ?source, ?channel_id, "Overfill");
        }

        added
    }

    pub fn push_blocking(
        &self,
        block: Arc<Block>,
        source: BlockSource,
    ) -> anyhow::Result<Result<SavedBlock, BlockError>> {
        let channel_id = ChannelId::LOOPBACK;

        let ctx = Arc::new(BlockContext::new(
            block.as_ref().clone(),
            source,
            channel_id,
        ));
        let waiter = ctx.get_waiter();
        self.push(ctx.clone());

        match waiter.wait_result() {
            Some(Ok(())) => Ok(Ok(ctx.saved_block.lock().unwrap().clone().unwrap())),
            Some(Err(e)) => Ok(Err(e)),
            None => {
                self.queue.lock().unwrap().timeout += 1;
                Err(anyhow!("Block dropped when processing"))
            }
        }
    }

    pub fn info(&self) -> FairQueueInfo<BlockSource> {
        self.queue.lock().unwrap().process_queue.info()
    }
}

impl Default for BlockProcessorQueue {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl DeadChannelCleanupStep for BlockProcessorQueue {
    fn clean_up_dead_channels(&self, dead_channel_ids: &[ChannelId]) {
        let mut guard = self.queue.lock().unwrap();
        for channel_id in dead_channel_ids {
            let iter = BlockSource::iter();
            for source in iter {
                guard.process_queue.remove(source, *channel_id)
            }
        }
    }
}

impl StatsSource for BlockProcessorQueue {
    fn collect_stats(&self, result: &mut StatsCollection) {
        self.queue.lock().unwrap().collect_stats(result)
    }
}

impl ContainerInfoProvider for BlockProcessorQueue {
    fn container_info(&self) -> ContainerInfo {
        self.queue.lock().unwrap().process_queue.container_info()
    }
}

struct BlockProcessorQueueImpl {
    process_queue: ProcessQueue,
    stopped: bool,
    cool_down: bool,
    processed: u64,
    overfill_count: u64,
    overfill_by_source: [u64; BlockSource::COUNT],
    timeout: u64,
}

impl BlockProcessorQueueImpl {
    pub fn new(config: ProcessQueueConfig) -> Self {
        Self {
            process_queue: ProcessQueue::new(config),
            stopped: false,
            cool_down: false,
            processed: 0,
            overfill_count: 0,
            overfill_by_source: Default::default(),
            timeout: 0,
        }
    }

    pub fn should_wait(&self) -> bool {
        if self.stopped {
            return false;
        }

        if self.cool_down {
            return true;
        }

        self.process_queue.is_empty()
    }

    pub fn push(&mut self, context: Arc<BlockContext>) -> bool {
        if self.stopped {
            return false;
        }

        let source = context.source;
        let added = self.process_queue.push(context);

        if added {
            self.processed += 1;
        } else {
            self.overfill_count += 1;
            self.overfill_by_source[source as usize] += 1;
        }

        added
    }
}

impl StatsSource for BlockProcessorQueueImpl {
    fn collect_stats(&self, result: &mut StatsCollection) {
        result.insert("block_processor", "process", self.processed);
        result.insert("block_processor", "overfill", self.overfill_count);
        // Current queue depth observed at collection time
        result.insert(
            "block_processor_queue",
            "size",
            self.process_queue.len() as u64,
        );
        for i in BlockSource::iter() {
            result.insert(
                "block_processor_overfill",
                i.into(),
                self.overfill_by_source[i as usize],
            );
        }
        result.insert("block_processor", "process_blocking_timeout", self.timeout);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ntest::timeout;

    #[test]
    fn enqueue() {
        let queue = BlockProcessorQueue::default();
        let ctx = test_block_context();

        queue.push(ctx);

        assert_eq!(queue.total_queue_len(), 1);
    }

    #[test]
    fn dequeue() {
        let queue = BlockProcessorQueue::default();
        let ctx = test_block_context();
        queue.push(ctx);

        let batch = queue.pop_blocking().unwrap();

        assert_eq!(batch.len(), 1);
    }

    #[test]
    #[timeout(5000)]
    fn pop_none_when_stopped() {
        let queue = BlockProcessorQueue::default();
        queue.stop();
        assert!(queue.pop_blocking().is_none());
    }

    #[test]
    #[timeout(5000)]
    fn can_be_nulled() {
        let queue = BlockProcessorQueue::new_null();
        queue.wait(Duration::MAX);
        assert!(queue.pop_blocking().is_none());
    }

    #[test]
    #[timeout(5000)]
    fn nulled_queue_returns_configured_response() {
        let ctx = test_block_context();
        let queue = BlockProcessorQueue::new_null_with(vec![ctx]);

        queue.pop_blocking().expect("should pop block");
        assert!(queue.pop_blocking().is_none());
        assert!(queue.pop_blocking().is_none());
        queue.wait(Duration::MAX);
    }

    #[test]
    #[timeout(5000)]
    fn can_track_waits() {
        let queue = BlockProcessorQueue::new_null();
        let waits = queue.track_waits();
        let duration = Duration::from_secs(42);

        queue.wait(duration);

        assert_eq!(waits.output(), vec![duration]);
    }

    fn test_block_context() -> Arc<BlockContext> {
        let block = Block::new_test_instance();
        BlockContext::new(block, BlockSource::Live, ChannelId::LOOPBACK).into()
    }
}
