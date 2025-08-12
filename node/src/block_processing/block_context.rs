use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use rsnano_core::{Block, SavedBlock};
use rsnano_ledger::BlockError;
use rsnano_network::ChannelId;

use super::BlockSource;

pub type BlockProcessorCallback = Box<dyn Fn(Result<(), BlockError>) + Send + Sync>;

pub struct BlockContext {
    pub block: Block,
    pub saved_block: Mutex<Option<SavedBlock>>,
    pub source: BlockSource,
    pub callback: Option<BlockProcessorCallback>,
    pub waiter: Arc<BlockProcessorWaiter>,
    pub channel_id: ChannelId,
    pub ingest_at: Instant,
}

impl BlockContext {
    pub fn new(block: Block, source: BlockSource, channel_id: ChannelId) -> Self {
        Self {
            block,
            saved_block: Mutex::new(None),
            source,
            callback: None,
            waiter: Arc::new(BlockProcessorWaiter::new()),
            channel_id,
            ingest_at: Instant::now(),
        }
    }

    pub fn new_with_callback(
        block: Block,
        source: BlockSource,
        channel_id: ChannelId,
        callback: BlockProcessorCallback,
    ) -> Self {
        Self {
            block,
            saved_block: Mutex::new(None),
            source,
            callback: Some(callback),
            waiter: Arc::new(BlockProcessorWaiter::new()),
            channel_id,
            ingest_at: Instant::now(),
        }
    }

    pub fn new_test_instance() -> Self {
        Self::new(
            Block::new_test_instance(),
            BlockSource::Live,
            ChannelId::LOOPBACK,
        )
    }

    pub fn set_result(&self, result: Result<(), BlockError>) {
        self.waiter.set_result(result);
    }

    pub fn get_waiter(&self) -> Arc<BlockProcessorWaiter> {
        self.waiter.clone()
    }
}

impl Drop for BlockContext {
    fn drop(&mut self) {
        self.waiter.cancel()
    }
}

pub struct BlockProcessorWaiter {
    result: Mutex<(Option<Result<(), BlockError>>, bool)>, // (status, done)
    condition: Condvar,
}

impl BlockProcessorWaiter {
    pub fn new() -> Self {
        Self {
            result: Mutex::new((None, false)),
            condition: Condvar::new(),
        }
    }

    pub fn set_result(&self, result: Result<(), BlockError>) {
        *self.result.lock().unwrap() = (Some(result), true);
        self.condition.notify_all();
    }

    pub fn cancel(&self) {
        self.result.lock().unwrap().1 = true;
        self.condition.notify_all();
    }

    pub fn wait_result(&self) -> Option<Result<(), BlockError>> {
        let guard = self.result.lock().unwrap();
        if guard.1 {
            return guard.0;
        }

        self.condition.wait_while(guard, |i| !i.1).unwrap().0
    }
}
