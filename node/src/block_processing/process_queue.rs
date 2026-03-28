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
            max_peer_queue: 500_000,
            max_system_queue: 500_000,
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
            BlockSource::Election | BlockSource::Forced => config.priority_system,
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

    pub fn remove(&mut self, source: BlockSource, channel_id: ChannelId) {
        self.queue.remove(&(source, channel_id));
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
}
