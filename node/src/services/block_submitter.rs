use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tracing::{trace, warn};

use rsnano_ledger::BlockError;
use rsnano_network::ChannelId;
use rsnano_types::{Block, BlockHash, SavedBlock};
use rsnano_utils::{
    fair_queue::FairQueueInfo,
    stats::{DetailType, StatType, Stats},
};
use rsnano_work::WorkThresholds;

use crate::block_processing::{BlockContext, BlockProcessorQueue, BlockSource};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockSubmissionError {
    NodeStopped,
    ProcessorStopped,
    QueueFull,
    Dropped,
    InsufficientWork,
}

impl std::fmt::Display for BlockSubmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockSubmissionError::NodeStopped => write!(f, "node is stopped"),
            BlockSubmissionError::ProcessorStopped => {
                write!(f, "Block processor is stopped")
            }
            BlockSubmissionError::QueueFull => write!(f, "Block processor queue is full"),
            BlockSubmissionError::Dropped => write!(f, "Block was dropped before processing"),
            BlockSubmissionError::InsufficientWork => write!(f, "Block work is insufficient"),
        }
    }
}

impl std::error::Error for BlockSubmissionError {}

impl From<BlockSubmissionError> for BlockError {
    fn from(err: BlockSubmissionError) -> Self {
        match err {
            BlockSubmissionError::InsufficientWork => BlockError::InsufficientWork,
            BlockSubmissionError::NodeStopped
            | BlockSubmissionError::ProcessorStopped
            | BlockSubmissionError::QueueFull
            | BlockSubmissionError::Dropped => BlockError::BadSignature,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BlockSubmission {
    pub block: Block,
    pub source: BlockSource,
    pub channel_id: ChannelId,
    pub validate_work: bool,
}

impl BlockSubmission {
    pub fn new(block: Block, source: BlockSource, channel_id: ChannelId) -> Self {
        Self {
            block,
            source,
            channel_id,
            validate_work: true,
        }
    }

    pub fn local(block: Block) -> Self {
        Self {
            block,
            source: BlockSource::Local,
            channel_id: ChannelId::LOOPBACK,
            validate_work: true,
        }
    }

    pub fn forced(block: Block) -> Self {
        Self {
            block,
            source: BlockSource::Forced,
            channel_id: ChannelId::LOOPBACK,
            validate_work: true,
        }
    }

    pub fn bootstrap(block: Block, channel_id: ChannelId) -> Self {
        Self {
            block,
            source: BlockSource::Bootstrap,
            channel_id,
            validate_work: false,
        }
    }

    pub fn with_work_validation(mut self, validate_work: bool) -> Self {
        self.validate_work = validate_work;
        self
    }
}

#[derive(Debug, Clone)]
pub struct BlockSubmissionResult {
    pub hash: BlockHash,
    pub status: Result<(), BlockError>,
    pub saved_block: Option<SavedBlock>,
}

#[derive(Clone)]
pub struct BlockPromise {
    context: Arc<BlockContext>,
}

impl BlockPromise {
    pub fn new(context: Arc<BlockContext>) -> Self {
        Self { context }
    }

    pub fn hash(&self) -> BlockHash {
        self.context.block.hash()
    }

    pub fn wait(&self) -> Result<BlockSubmissionResult, BlockSubmissionError> {
        match self.context.waiter.wait_result() {
            Some(status) => Ok(BlockSubmissionResult {
                hash: self.context.block.hash(),
                status,
                saved_block: self.context.saved_block.lock().unwrap().clone(),
            }),
            None => Err(BlockSubmissionError::Dropped),
        }
    }
}

impl std::fmt::Debug for BlockPromise {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockPromise")
            .field("hash", &self.hash())
            .finish()
    }
}

#[derive(Clone)]
pub struct BlockSubmitter {
    queue: Arc<BlockProcessorQueue>,
    stats: Arc<Stats>,
    stopped: Arc<AtomicBool>,
    work_thresholds: WorkThresholds,
}

impl BlockSubmitter {
    pub fn new(
        queue: Arc<BlockProcessorQueue>,
        stats: Arc<Stats>,
        stopped: Arc<AtomicBool>,
        work_thresholds: WorkThresholds,
    ) -> Self {
        Self {
            queue,
            stats,
            stopped,
            work_thresholds,
        }
    }

    pub fn new_null() -> Self {
        Self::new(
            Arc::new(BlockProcessorQueue::new_null()),
            Arc::new(Stats::default()),
            Arc::new(AtomicBool::new(false)),
            WorkThresholds::none(),
        )
    }

    pub fn submit(
        &self,
        submission: BlockSubmission,
    ) -> Result<BlockPromise, BlockSubmissionError> {
        self.validate(&submission)?;

        let context = Arc::new(BlockContext::new(
            submission.block,
            submission.source,
            submission.channel_id,
        ));

        let added = self.queue.push(context.clone());
        if !added {
            if self.queue.stopped() {
                self.stats
                    .inc(StatType::BlockProcessor, DetailType::Ignored);
                return Err(BlockSubmissionError::ProcessorStopped);
            }

            self.stats
                .inc(StatType::BlockProcessor, DetailType::Overfill);
            return Err(BlockSubmissionError::QueueFull);
        }

        trace!(
            block_hash = %context.block.hash(),
            source = ?context.source,
            channel = ?context.channel_id,
            "submitted block to processor"
        );
        self.stats.inc(StatType::BlockProcessor, DetailType::Queue);

        Ok(BlockPromise::new(context))
    }

    pub fn submit_and_wait(
        &self,
        submission: BlockSubmission,
    ) -> Result<BlockSubmissionResult, BlockSubmissionError> {
        let promise = self.submit(submission)?;
        promise.wait()
    }

    pub fn submit_local(
        &self,
        block: Block,
    ) -> Result<BlockSubmissionResult, BlockSubmissionError> {
        self.submit_and_wait(BlockSubmission::local(block))
    }

    pub fn submit_live(
        &self,
        block: Block,
        channel_id: ChannelId,
    ) -> Result<(), BlockSubmissionError> {
        self.submit(BlockSubmission::new(block, BlockSource::Live, channel_id))
            .map(|_| ())
    }

    pub fn submit_live_originator(
        &self,
        block: Block,
        channel_id: ChannelId,
    ) -> Result<(), BlockSubmissionError> {
        self.submit(BlockSubmission::new(
            block,
            BlockSource::LiveOriginator,
            channel_id,
        ))
        .map(|_| ())
    }

    pub fn submit_bootstrap(
        &self,
        block: Block,
        channel_id: ChannelId,
    ) -> Result<(), BlockSubmissionError> {
        self.submit(BlockSubmission::bootstrap(block, channel_id))
            .map(|_| ())
    }

    pub fn submit_forced(&self, block: Block) -> Result<(), BlockSubmissionError> {
        self.submit(BlockSubmission::forced(block)).map(|_| ())
    }

    pub fn submit_without_work_validation(
        &self,
        block: Block,
        source: BlockSource,
        channel_id: ChannelId,
    ) -> Result<(), BlockSubmissionError> {
        self.submit(BlockSubmission::new(block, source, channel_id).with_work_validation(false))
            .map(|_| ())
    }

    pub fn submit_and_wait_with_source(
        &self,
        block: Block,
        source: BlockSource,
        channel_id: ChannelId,
    ) -> Result<BlockSubmissionResult, BlockSubmissionError> {
        self.submit_and_wait(BlockSubmission::new(block, source, channel_id))
    }

    pub fn submit_with_source(
        &self,
        block: Block,
        source: BlockSource,
        channel_id: ChannelId,
    ) -> Result<(), BlockSubmissionError> {
        self.submit(BlockSubmission::new(block, source, channel_id))
            .map(|_| ())
    }

    pub fn queue_len(&self, source: BlockSource) -> usize {
        self.queue.queue_len(source)
    }

    pub fn queue_info(&self) -> FairQueueInfo<BlockSource> {
        self.queue.info()
    }

    fn validate(&self, submission: &BlockSubmission) -> Result<(), BlockSubmissionError> {
        if self.stopped.load(Ordering::SeqCst) {
            warn!(block_hash = %submission.block.hash(), "rejecting block submission because node is stopped");
            return Err(BlockSubmissionError::NodeStopped);
        }

        if submission.validate_work && !self.work_thresholds.validate_entry_block(&submission.block)
        {
            self.stats
                .inc(StatType::BlockProcessor, DetailType::InsufficientWork);
            return Err(BlockSubmissionError::InsufficientWork);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_processing::ProcessQueueConfig;
    use rsnano_utils::stats::Direction;

    fn submitter_with(queue: Arc<BlockProcessorQueue>, stopped: Arc<AtomicBool>) -> BlockSubmitter {
        BlockSubmitter::new(
            queue,
            Arc::new(Stats::default()),
            stopped,
            WorkThresholds::none(),
        )
    }

    #[test]
    fn rejects_when_stopped() {
        let stopped = Arc::new(AtomicBool::new(true));
        let submitter = submitter_with(Arc::new(BlockProcessorQueue::default()), stopped);
        let block = Block::new_test_instance();

        let err = submitter.submit(BlockSubmission::local(block));

        assert_eq!(err.unwrap_err(), BlockSubmissionError::NodeStopped);
    }

    #[test]
    fn rejects_invalid_work() {
        let stopped = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Stats::default());
        let submitter = BlockSubmitter::new(
            Arc::new(BlockProcessorQueue::default()),
            stats.clone(),
            stopped,
            WorkThresholds::impossible(),
        );
        let block = Block::new_test_instance();

        let err = submitter.submit(BlockSubmission::local(block));

        assert_eq!(err.unwrap_err(), BlockSubmissionError::InsufficientWork);
        assert_eq!(
            stats.count(
                StatType::BlockProcessor,
                DetailType::InsufficientWork,
                Direction::In,
            ),
            1
        );
    }

    #[test]
    fn returns_processor_stopped_when_queue_stopped() {
        let stopped = Arc::new(AtomicBool::new(false));
        let queue = Arc::new(BlockProcessorQueue::new_null());
        let submitter = submitter_with(queue, stopped);
        let block = Block::new_test_instance();

        let err = submitter.submit(BlockSubmission::local(block));

        assert_eq!(err.unwrap_err(), BlockSubmissionError::ProcessorStopped);
    }

    #[test]
    fn waits_for_result() {
        let stopped = Arc::new(AtomicBool::new(false));
        let submitter = submitter_with(Arc::new(BlockProcessorQueue::default()), stopped);
        let block = Block::new_test_instance();

        let promise = submitter
            .submit(BlockSubmission::local(block.clone()))
            .unwrap();

        promise.context.waiter.set_result(Ok(()));
        *promise.context.saved_block.lock().unwrap() =
            Some(SavedBlock::new_test_instance_with(block.clone()));

        let result = promise.wait().unwrap();

        assert!(result.status.is_ok());
        assert!(result.saved_block.is_some());
    }

    #[test]
    fn submit_live_originator_enqueues_live_originator_source() {
        let stopped = Arc::new(AtomicBool::new(false));
        let queue = Arc::new(BlockProcessorQueue::new(ProcessQueueConfig::default()));
        let submitter = Arc::new(BlockSubmitter::new(
            queue.clone(),
            Arc::new(Stats::default()),
            stopped,
            WorkThresholds::none(),
        ));

        let block = Block::new_test_instance();
        submitter
            .submit_live_originator(block, ChannelId::LOOPBACK)
            .unwrap();

        assert_eq!(queue.queue_len(BlockSource::LiveOriginator), 1);
    }

    #[test]
    fn submit_bootstrap_bypasses_work_validation() {
        let stopped = Arc::new(AtomicBool::new(false));
        let queue = Arc::new(BlockProcessorQueue::new(ProcessQueueConfig::default()));
        let submitter = Arc::new(BlockSubmitter::new(
            queue.clone(),
            Arc::new(Stats::default()),
            stopped,
            WorkThresholds::impossible(),
        ));
        let block = Block::new_test_instance();

        submitter
            .submit_bootstrap(block, ChannelId::LOOPBACK)
            .expect("bootstrap submission should bypass work validation");

        assert_eq!(queue.queue_len(BlockSource::Bootstrap), 1);
    }
}
