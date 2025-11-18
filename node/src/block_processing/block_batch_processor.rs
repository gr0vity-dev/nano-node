use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
};

use strum::{EnumCount, IntoEnumIterator};
use tracing::{trace, warn};

use rsnano_ledger::{BlockError, Ledger};
use rsnano_nullable_clock::SteadyClock;
use rsnano_utils::{
    stats::{StatsCollection, StatsSource},
    sync::backpressure_channel::{Sender, channel},
};
use store_rocksdb::default_ledger_store_factory;

use super::{BlockContext, BlockSource, LedgerEvent, UncheckedBlockReenqueuer, UncheckedMap};
use crate::block_processing::ProcessedResult;

pub(crate) struct BlockBatchProcessor {
    pub ledger: Arc<Ledger>,
    pub unchecked: Arc<Mutex<UncheckedMap>>,
    pub stats: Arc<BlockBatchProcessorStats>,
    pub event_publisher: Sender<LedgerEvent>,
    pub unchecked_reenqueuer: UncheckedBlockReenqueuer,
    pub clock: Arc<SteadyClock>,
}

impl BlockBatchProcessor {
    #[allow(dead_code)]
    pub fn new_null() -> Self {
        Self {
            ledger: Arc::new(Ledger::new_null(default_ledger_store_factory())),
            unchecked: Arc::new(Mutex::new(UncheckedMap::default())),
            stats: Arc::new(BlockBatchProcessorStats::default()),
            event_publisher: channel(0).0,
            unchecked_reenqueuer: UncheckedBlockReenqueuer::new_null(),
            clock: Arc::new(SteadyClock::new_null()),
        }
    }

    pub(crate) fn process_blocks(&mut self, mut batch: VecDeque<Arc<BlockContext>>) {
        let now = self.clock.now();

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

            let hash = block_ctx.block.hash();
            let block = &block_ctx.block;

            match status {
                Ok(()) => {
                    trace!(block_hash = %hash, "Block processed");
                    self.unchecked_reenqueuer
                        .enqueue_blocks_with_dependency(hash);
                }
                Err(error) => {
                    trace!(block_hash = %hash, ?error, "Block processing failed");
                    match error {
                        BlockError::GapPrevious => {
                            self.unchecked.lock().unwrap().put(
                                block.previous(),
                                block.clone(),
                                now,
                            );
                        }
                        BlockError::GapSource => {
                            self.unchecked.lock().unwrap().put(
                                block.source_or_link(),
                                block.clone(),
                                now,
                            );
                        }
                        _ => {}
                    }
                }
            }
        }

        // Set results for futures when not holding the lock
        for (res, context) in result.iter_mut() {
            if let Some(cb) = &context.callback {
                let saved_block = context.saved_block.lock().unwrap().clone();
                (cb)(&context.block.hash(), *res, saved_block.as_ref());
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

#[derive(Default)]
pub(crate) struct BlockBatchProcessorStats {
    progress: AtomicU64,
    errors: [AtomicU64; BlockError::COUNT],
    sources: [AtomicU64; BlockSource::COUNT],
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
    }
}
