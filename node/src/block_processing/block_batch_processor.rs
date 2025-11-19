use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
};

use strum::{EnumCount, IntoEnumIterator};
use tracing::{trace, warn};

use crate::ledger_factory::default_ledger_store_factory;
use rsnano_ledger::{
    BatchProcessEntry, BlockError, Ledger, WriterType, DEFAULT_OPTIMISTIC_RETRIES,
};
use rsnano_nullable_clock::SteadyClock;
use rsnano_utils::{
    stats::{StatsCollection, StatsSource},
    sync::backpressure_channel::{Sender, channel},
};

use super::{BlockContext, BlockSource, LedgerEvent, UncheckedBlockReenqueuer, UncheckedMap};
use crate::block_processing::ProcessedResult;

pub struct BlockBatchProcessor {
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

    pub fn process_blocks(&mut self, mut batch: VecDeque<Arc<BlockContext>>) {
        let now = self.clock.now();

        self.roll_back_competitor_blocks(&batch);

        let validation_results = self.ledger.validate_batch(batch.iter().map(|c| &c.block));
        let processed_entries = self
            .ledger
            .tx_optimistic_process(
                WriterType::BlockProcessor,
                DEFAULT_OPTIMISTIC_RETRIES,
                |txn, deferred| {
                    let (processed, inserted_hashes) =
                        self.ledger
                            .apply_validated_batch(txn, deferred, &validation_results);
                    Ok((processed, inserted_hashes))
                },
            )
            .unwrap_or_else(|e| panic!("failed to process block batch: {e}"));
        self.stats
            .optimistic_successes
            .store(self.ledger.optimistic_successes(), Relaxed);
        self.stats
            .optimistic_conflicts
            .store(self.ledger.optimistic_conflicts(), Relaxed);
        self.stats
            .pessimistic_fallbacks
            .store(self.ledger.pessimistic_fallbacks(), Relaxed);

        let processed_result: Vec<_> = processed_entries
            .iter()
            .zip(&batch)
            .map(|(entry, ctx)| ProcessedResult {
                block: ctx.block.clone(),
                source: ctx.source,
                status: entry.status,
                saved_block: entry.saved_block.clone(),
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

        assert_eq!(processed_entries.len(), batch.len());
        let mut result: Vec<(BatchProcessEntry, Arc<BlockContext>)> = processed_entries
            .into_iter()
            .zip(batch.drain(..))
            .map(|(entry, block_ctx)| {
                if entry.saved_block.is_some() {
                    *block_ctx.saved_block.lock().unwrap() = entry.saved_block.clone();
                }
                (entry, block_ctx)
            })
            .collect();

        for (entry, block_ctx) in result.iter().rev() {
            match &entry.status {
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

            match &entry.status {
                Ok(()) => {
                    trace!(block_hash = %hash, "Block processed");
                    self.unchecked_reenqueuer
                        .enqueue_blocks_with_dependency(hash);
                    if entry.inserted {
                        self.ledger
                            .record_block_insert_source(block_ctx.source.as_u8(), hash);
                    } else if entry.preexisting {
                        self.ledger
                            .record_duplicate_insert_source(block_ctx.source.as_u8(), hash);
                    }
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

        for (entry, context) in result.iter_mut() {
            if let Some(cb) = &context.callback {
                let saved_block = context.saved_block.lock().unwrap().clone();
                (cb)(&context.block.hash(), entry.status, saved_block.as_ref());
            }
            context.set_result(entry.status);
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
pub struct BlockBatchProcessorStats {
    progress: AtomicU64,
    errors: [AtomicU64; BlockError::COUNT],
    sources: [AtomicU64; BlockSource::COUNT],
    optimistic_successes: AtomicU64,
    optimistic_conflicts: AtomicU64,
    pessimistic_fallbacks: AtomicU64,
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

        result.insert(
            "block_processor_writer",
            "optimistic_successes",
            self.optimistic_successes.load(Relaxed),
        );
        result.insert(
            "block_processor_writer",
            "optimistic_conflicts",
            self.optimistic_conflicts.load(Relaxed),
        );
        result.insert(
            "block_processor_writer",
            "pessimistic_fallbacks",
            self.pessimistic_fallbacks.load(Relaxed),
        );
    }
}

impl BlockBatchProcessorStats {
    pub fn optimistic_successes(&self) -> u64 {
        self.optimistic_successes.load(Relaxed)
    }

    pub fn optimistic_conflicts(&self) -> u64 {
        self.optimistic_conflicts.load(Relaxed)
    }

    pub fn pessimistic_fallbacks(&self) -> u64 {
        self.pessimistic_fallbacks.load(Relaxed)
    }
}
