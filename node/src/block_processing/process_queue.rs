use std::{
    collections::VecDeque,
    ops::{Deref, DerefMut},
    sync::Arc,
};

use rsnano_network::ChannelId;
use rsnano_types::Block;
use rsnano_utils::{
    container_info::ContainerInfo,
    fair_queue::{FairQueue, FairQueueInfo},
};

use super::BlockContext;
use rsnano_ledger::BlockSource;

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessQueueConfig {
    // Maximum number of blocks to queue from network peers
    pub max_peer_queue: usize,

    // Maximum number of blocks to queue from system components (local RPC, bootstrap)
    pub max_system_queue: usize,

    // Higher priority gets processed more frequently
    pub priority_live: usize,
    pub priority_bootstrap: usize,
    pub priority_local: usize,
    pub priority_system: usize,
    pub batch_size: usize,
}

impl ProcessQueueConfig {}

impl Default for ProcessQueueConfig {
    fn default() -> Self {
        Self {
            max_peer_queue: 1024,
            max_system_queue: 16 * 1024,
            priority_live: 1,
            priority_bootstrap: 8,
            priority_local: 16,
            priority_system: 32,
            batch_size: 256,
        }
    }
}

pub(crate) struct ProcessQueue {
    queue: FairQueue<(BlockSource, ChannelId), Arc<BlockContext>>,
    batch_size: usize,
}

impl ProcessQueue {
    pub fn new(config: ProcessQueueConfig) -> Self {
        let config_l = config.clone();
        let max_size_query = move |origin: &(BlockSource, ChannelId)| match origin.0 {
            BlockSource::Live | BlockSource::LiveOriginator => config_l.max_peer_queue,
            _ => config_l.max_system_queue,
        };

        let config_l = config.clone();
        let priority_query = move |origin: &(BlockSource, ChannelId)| match origin.0 {
            BlockSource::Live | BlockSource::LiveOriginator => config.priority_live,
            BlockSource::Bootstrap | BlockSource::Unchecked => config_l.priority_bootstrap,
            BlockSource::Local => config_l.priority_local,
            BlockSource::Forced => config.priority_system,
        };

        Self {
            queue: FairQueue::new(max_size_query, priority_query),
            batch_size: config.batch_size,
        }
    }

    pub fn push(&mut self, context: Arc<BlockContext>) -> bool {
        let source = context.source;
        let channel_id = context.channel_id;
        self.queue.push((source, channel_id), context)
    }

    pub fn next_batch(&mut self) -> VecDeque<Arc<BlockContext>> {
        let mut results = VecDeque::new();
        while !self.is_empty() && results.len() < self.batch_size {
            results.push_back(self.next());
        }
        results
    }

    fn next(&mut self) -> Arc<BlockContext> {
        if !self.queue.is_empty() {
            let ((source, _), request) = self.queue.pop().unwrap();
            assert!(source != BlockSource::Forced || request.source == BlockSource::Forced);
            return request;
        }

        panic!("next() called when no blocks are ready");
    }

    pub fn source_len(&self, source: BlockSource) -> usize {
        self.queue
            .sum_queue_len((source, ChannelId::MIN)..=(source, ChannelId::MAX))
    }

    pub fn info(&self) -> FairQueueInfo<BlockSource> {
        self.compacted_info(|(source, _)| *source)
    }

    pub fn container_info(&self) -> ContainerInfo {
        ContainerInfo::builder()
            .leaf("blocks", self.queue.len(), size_of::<Arc<Block>>())
            .leaf(
                "forced",
                self.queue
                    .queue_len(&(BlockSource::Forced, ChannelId::LOOPBACK)),
                size_of::<Arc<Block>>(),
            )
            .node("queue", self.queue.container_info())
            .finish()
    }
}

impl Deref for ProcessQueue {
    type Target = FairQueue<(BlockSource, ChannelId), Arc<BlockContext>>;

    fn deref(&self) -> &Self::Target {
        &self.queue
    }
}

impl DerefMut for ProcessQueue {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.queue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = ProcessQueueConfig::default();
        assert_eq!(config.max_system_queue, 1024 * 16, "max system queue");
        assert_eq!(config.max_peer_queue, 1024, "max peer queue");
    }

    #[test]
    fn competing_sources_are_dequeued_by_configured_service_share() {
        let mut queue = ProcessQueue::new(ProcessQueueConfig {
            priority_live: 1,
            priority_bootstrap: 2,
            priority_local: 3,
            priority_system: 4,
            batch_size: 10,
            ..ProcessQueueConfig::default()
        });

        push_blocks(&mut queue, BlockSource::Live, 10);
        push_blocks(&mut queue, BlockSource::Bootstrap, 10);
        push_blocks(&mut queue, BlockSource::Local, 10);
        push_blocks(&mut queue, BlockSource::Forced, 10);

        let sources = pop_sources(&mut queue, 10);

        assert_eq!(
            sources,
            vec![
                BlockSource::Live,
                BlockSource::Bootstrap,
                BlockSource::Bootstrap,
                BlockSource::Local,
                BlockSource::Local,
                BlockSource::Local,
                BlockSource::Forced,
                BlockSource::Forced,
                BlockSource::Forced,
                BlockSource::Forced,
            ]
        );
    }

    #[test]
    fn live_peer_queue_can_fill_before_system_queue() {
        let mut queue = ProcessQueue::new(ProcessQueueConfig {
            max_peer_queue: 2,
            max_system_queue: 4,
            ..ProcessQueueConfig::default()
        });

        assert!(queue.push(block_context(BlockSource::Live)));
        assert!(queue.push(block_context(BlockSource::Live)));
        assert!(!queue.push(block_context(BlockSource::Live)));

        assert!(queue.push(block_context(BlockSource::Bootstrap)));
        assert!(queue.push(block_context(BlockSource::Bootstrap)));
        assert!(queue.push(block_context(BlockSource::Bootstrap)));
        assert!(queue.push(block_context(BlockSource::Bootstrap)));
        assert!(!queue.push(block_context(BlockSource::Bootstrap)));

        assert_eq!(queue.source_len(BlockSource::Live), 2);
        assert_eq!(queue.source_len(BlockSource::Bootstrap), 4);
    }

    #[test]
    fn pressure_live_dequeue_distance_under_bootstrap_and_unchecked_backlog() {
        let mut queue = ProcessQueue::new(ProcessQueueConfig {
            batch_size: 18,
            ..ProcessQueueConfig::default()
        });

        push_blocks(&mut queue, BlockSource::Live, 2);
        push_blocks(&mut queue, BlockSource::Bootstrap, 16);
        push_blocks(&mut queue, BlockSource::Unchecked, 16);

        let sources = pop_sources(&mut queue, 18);

        assert_eq!(positions_of(&sources, BlockSource::Live), vec![0, 17]);
        assert_eq!(count_source(&sources, BlockSource::Live), 2);
        assert_eq!(count_source(&sources, BlockSource::Bootstrap), 8);
        assert_eq!(count_source(&sources, BlockSource::Unchecked), 8);
        assert_eq!(queue.source_len(BlockSource::Bootstrap), 8);
        assert_eq!(queue.source_len(BlockSource::Unchecked), 8);
    }

    #[test]
    fn pressure_forced_system_share_creates_the_longest_default_live_gap() {
        let mut queue = ProcessQueue::new(ProcessQueueConfig {
            batch_size: 34,
            ..ProcessQueueConfig::default()
        });

        push_blocks(&mut queue, BlockSource::Live, 2);
        push_blocks(&mut queue, BlockSource::Forced, 64);

        let sources = pop_sources(&mut queue, 34);

        assert_eq!(positions_of(&sources, BlockSource::Live), vec![0, 33]);
        assert_eq!(count_source(&sources, BlockSource::Live), 2);
        assert_eq!(count_source(&sources, BlockSource::Forced), 32);
        assert_eq!(queue.source_len(BlockSource::Forced), 32);
    }

    #[test]
    fn pressure_live_and_live_originator_get_separate_peer_fair_queue_turns() {
        let mut queue = ProcessQueue::new(ProcessQueueConfig {
            max_peer_queue: 2,
            batch_size: 4,
            ..ProcessQueueConfig::default()
        });

        push_blocks(&mut queue, BlockSource::Live, 2);
        push_blocks(&mut queue, BlockSource::LiveOriginator, 2);
        assert!(!queue.push(block_context(BlockSource::Live)));
        assert!(!queue.push(block_context(BlockSource::LiveOriginator)));

        let sources = pop_sources(&mut queue, 4);

        assert_eq!(
            sources,
            vec![
                BlockSource::Live,
                BlockSource::LiveOriginator,
                BlockSource::Live,
                BlockSource::LiveOriginator,
            ]
        );
    }

    /* Test helpers */

    fn push_blocks(queue: &mut ProcessQueue, source: BlockSource, count: usize) {
        for _ in 0..count {
            assert!(queue.push(block_context(source)));
        }
    }

    fn pop_sources(queue: &mut ProcessQueue, count: usize) -> Vec<BlockSource> {
        (0..count).map(|_| queue.next().source).collect()
    }

    fn positions_of(sources: &[BlockSource], source: BlockSource) -> Vec<usize> {
        sources
            .iter()
            .enumerate()
            .filter_map(|(i, s)| (*s == source).then_some(i))
            .collect()
    }

    fn count_source(sources: &[BlockSource], source: BlockSource) -> usize {
        sources.iter().filter(|s| **s == source).count()
    }

    fn block_context(source: BlockSource) -> Arc<BlockContext> {
        Arc::new(BlockContext::new(
            Block::new_test_instance(),
            source,
            ChannelId::LOOPBACK,
        ))
    }
}
