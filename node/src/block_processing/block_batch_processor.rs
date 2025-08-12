use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use strum::{EnumCount, IntoEnumIterator};
use tracing::{debug, warn};

use rsnano_core::{
    utils::{backpressure_channel, BackpressureSender},
    BlockType, Epoch, UncheckedInfo,
};
use rsnano_ledger::{BlockError, Ledger};
use rsnano_stats::{StatsCollection, StatsSource};
use bounded_vec_deque::BoundedVecDeque;

use super::{BlockContext, BlockSource, LedgerEvent, UncheckedMap};
use crate::block_processing::ProcessedResult;

pub(crate) struct BlockBatchProcessor {
    pub ledger: Arc<Ledger>,
    pub unchecked: Arc<UncheckedMap>,
    pub stats: Arc<BlockBatchProcessorStats>,
    pub event_publisher: BackpressureSender<LedgerEvent>,
}

impl BlockBatchProcessor {
    #[allow(dead_code)]
    pub fn new_null() -> Self {
        Self {
            ledger: Arc::new(Ledger::new_null()),
            unchecked: Arc::new(UncheckedMap::default()),
            stats: Arc::new(BlockBatchProcessorStats::default()),
            event_publisher: backpressure_channel(0).0,
        }
    }

    pub(crate) fn process_blocks(&self, mut batch: VecDeque<Arc<BlockContext>>) {
        let timer = Instant::now();
        let now = Instant::now();

        // Record queue wait time for each block
        for ctx in batch.iter() {
            let wait_ms = now
                .saturating_duration_since(ctx.ingest_at)
                .as_millis() as u64;
            self.stats.add_queue_wait(wait_ms);
        }

        self.roll_back_competitor_blocks(&batch);

        let mut result = self.ledger.process_batch(batch.iter().map(|c| &c.block));

        let processed_result: Vec<_> = result
            .processed
            .iter()
            .zip(&batch)
            .map(|((result, block), ctx)| ProcessedResult {
                block: ctx.block.clone(),
                source: ctx.source,
                status: *result,
                saved_block: block.clone(),
            })
            .collect();

        if !processed_result.is_empty() {
            if let Err(e) = self
                .event_publisher
                .send(LedgerEvent::BlocksProcessed(processed_result))
            {
                warn!("Failed to publish blocks processed event: {e:?}");
            }
        }

        if result.processed.len() > 0 && timer.elapsed() > Duration::from_millis(100) {
            debug!(
                "Processed {} blocks in {} ms",
                result.processed.len(),
                timer.elapsed().as_millis(),
            );
        }

        assert_eq!(result.processed.len(), batch.len());
        let mut result: Vec<(Result<(), BlockError>, Arc<BlockContext>)> = result
            .processed
            .drain(..)
            .zip(batch.drain(..))
            .map(|((status, saved_block), block_ctx)| {
                if saved_block.is_some() {
                    *block_ctx.saved_block.lock().unwrap() = saved_block;
                }

                (status, block_ctx)
            })
            .collect();

        // Iterate in reverse order so that when consecutive blocks where processed with
        // gap_previous, that the successful insert of the first block is processed last
        // and the unchecked_map trigger succeeds.
        for (status, block_ctx) in result.iter().rev() {
            match status {
                Ok(()) => {
                    self.stats.progress.fetch_add(1, Relaxed);
                }
                Err(e) => {
                    self.stats.errors[*e as usize].fetch_add(1, Relaxed);
                }
            }

            self.stats.sources[block_ctx.source as usize].fetch_add(1, Relaxed);

            let hash = &block_ctx.block.hash();
            let block = &block_ctx.block;
            let saved_block = block_ctx.saved_block.lock().unwrap().clone();

            match status {
                Ok(()) => {
                    self.unchecked.trigger(&hash.into());

                    /*
                     * For send blocks check epoch open unchecked (gap pending).
                     * For state blocks check only send subtype and only if block epoch is not last epoch.
                     * If epoch is last, then pending entry shouldn't trigger same epoch open block for destination account.
                     * */
                    let block = saved_block.unwrap();
                    if block.block_type() == BlockType::LegacySend
                        || block.block_type() == BlockType::State
                            && block.is_send()
                            && block.epoch() < Epoch::MAX
                    {
                        self.unchecked.trigger(&block.destination_or_link().into());
                    }
                }
                Err(BlockError::GapPrevious) => {
                    self.unchecked
                        .put(block.previous().into(), UncheckedInfo::new(block.clone()));
                }
                Err(BlockError::GapSource) => {
                    self.unchecked.put(
                        block.source_or_link().into(),
                        UncheckedInfo::new(block.clone()),
                    );
                }
                Err(BlockError::GapEpochOpenPending) => {
                    // Specific unchecked key starting with epoch open block account public key
                    self.unchecked.put(
                        block.account_field().unwrap().into(),
                        UncheckedInfo::new(block.clone()),
                    );
                }
                Err(BlockError::Old) => {
                    debug!("Block is old: {}", hash)
                }
                Err(BlockError::Conflict) => {
                    debug!("Block conflict: {}", hash)
                }
                // These are unexpected and indicate erroneous/malicious behavior, log debug info to highlight the issue
                Err(BlockError::BadSignature) => {
                    debug!("Block signature is invalid: {}", hash)
                }
                Err(BlockError::NegativeSpend) => {
                    debug!("Block spends negative amount: {}", hash)
                }
                Err(BlockError::Unreceivable) => {
                    debug!("Block is unreceivable: {}", hash)
                }
                Err(BlockError::Fork) => {
                    debug!("Block is a fork: {}", hash)
                }
                Err(BlockError::OpenedBurnAccount) => {
                    debug!("Block opens burn account: {}", hash)
                }
                Err(BlockError::BalanceMismatch) => {
                    debug!("Block balance mismatch: {}", hash)
                }
                Err(BlockError::RepresentativeMismatch) => {
                    debug!("Block representative mismatch: {}", hash)
                }
                Err(BlockError::BlockPosition) => {
                    debug!("Block is in incorrect position: {}", hash)
                }
                Err(BlockError::InsufficientWork) => {
                    debug!("Block has insufficient work: {}", hash)
                }
            }
        }

        // Record average processing time per block and batch elapsed for this batch
        let processed_count = result.len() as u64;
        let elapsed_ms = timer.elapsed().as_millis() as u64;
        if processed_count > 0 {
            let per_block_ms = elapsed_ms / processed_count;
            self.stats.add_process_time_ms(per_block_ms);
            self.stats.add_insert_batch_ms(elapsed_ms);
        }

        // Set results for futures when not holding the lock
        for (res, context) in result.iter_mut() {
            if let Some(cb) = &context.callback {
                cb(*res);
            }
            context.set_result(*res);
        }
    }

    fn roll_back_competitor_blocks(&self, batch: &VecDeque<Arc<BlockContext>>) {
        let fork_blocks = batch.iter().filter_map(|i| {
            if i.source == BlockSource::Forced {
                Some(&i.block)
            } else {
                None
            }
        });
        self.ledger.roll_back_competitors(fork_blocks, |results| {
            if let Err(e) = self
                .event_publisher
                .send(LedgerEvent::BlocksRolledBack(results))
            {
                warn!("Failed to publish rolled back event: {e:?}");
            }
        });
    }
}

pub(crate) struct BlockBatchProcessorStats {
    progress: AtomicU64,
    errors: [AtomicU64; BlockError::COUNT],
    sources: [AtomicU64; BlockSource::COUNT],
    queue_wait_ms: Mutex<BoundedVecDeque<u64>>, // last N queue wait times in ms
    process_time_ms: Mutex<BoundedVecDeque<u64>>, // last N per-block processing times in ms
    insert_batch_ms: Mutex<BoundedVecDeque<u64>>, // last N insert-batch elapsed times in ms
}

impl StatsSource for BlockBatchProcessorStats {
    fn collect_stats(&self, result: &mut StatsCollection) {
        result.insert(
            "block_processor_result",
            "progress",
            self.progress.load(Relaxed),
        );

        for e in BlockError::iter() {
            result.insert(
                "block_processor_result",
                e.into(),
                self.errors[e as usize].load(Relaxed),
            );
        }

        for s in BlockSource::iter() {
            result.insert(
                "block_processor_source",
                s.into(),
                self.sources[s as usize].load(Relaxed),
            );
        }

        // Latency percentiles for queue wait and processing time
        fn publish_percentiles(
            result: &mut StatsCollection,
            samples: &Mutex<BoundedVecDeque<u64>>,
            p50_detail: &'static str,
            p95_detail: &'static str,
            p99_detail: &'static str,
        ) {
            let mut vec: Vec<u64> = {
                let guard = samples.lock().unwrap();
                guard.iter().cloned().collect()
            };
            if vec.is_empty() {
                result.insert("block_pipeline_latency", p50_detail, 0u64);
                result.insert("block_pipeline_latency", p95_detail, 0u64);
                result.insert("block_pipeline_latency", p99_detail, 0u64);
                return;
            }
            vec.sort_unstable();
            let pct = |p: usize| -> u64 {
                let idx = (vec.len() * p) / 100;
                vec[vec.len().saturating_sub(1).min(idx)]
            };
            result.insert("block_pipeline_latency", p50_detail, pct(50));
            result.insert("block_pipeline_latency", p95_detail, pct(95));
            result.insert("block_pipeline_latency", p99_detail, pct(99));
        }

        publish_percentiles(
            result,
            &self.queue_wait_ms,
            "queue_wait_ms_p50",
            "queue_wait_ms_p95",
            "queue_wait_ms_p99",
        );
        publish_percentiles(
            result,
            &self.process_time_ms,
            "process_time_ms_p50",
            "process_time_ms_p95",
            "process_time_ms_p99",
        );

        publish_percentiles(
            result,
            &self.insert_batch_ms,
            "insert_batch_ms_p50",
            "insert_batch_ms_p95",
            "insert_batch_ms_p99",
        );
    }
}

impl Default for BlockBatchProcessorStats {
    fn default() -> Self {
        use std::array::from_fn;
        Self {
            progress: AtomicU64::new(0),
            errors: from_fn(|_| AtomicU64::new(0)),
            sources: from_fn(|_| AtomicU64::new(0)),
            queue_wait_ms: Mutex::new(BoundedVecDeque::new(1000)),
            process_time_ms: Mutex::new(BoundedVecDeque::new(1000)),
            insert_batch_ms: Mutex::new(BoundedVecDeque::new(1000)),
        }
    }
}

impl BlockBatchProcessorStats {
    pub fn add_queue_wait(&self, wait_ms: u64) {
        let mut guard = self.queue_wait_ms.lock().unwrap();
        guard.push_back(wait_ms);
    }

    pub fn add_process_time_ms(&self, ms: u64) {
        let mut guard = self.process_time_ms.lock().unwrap();
        guard.push_back(ms);
    }

    pub fn add_insert_batch_ms(&self, ms: u64) {
        let mut guard = self.insert_batch_ms.lock().unwrap();
        guard.push_back(ms);
    }
}
