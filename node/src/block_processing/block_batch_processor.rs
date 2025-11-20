use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::Instant,
};

use strum::{EnumCount, IntoEnumIterator};
use tracing::{trace, warn};

use crate::ledger_factory::default_ledger_store_factory;
use rsnano_ledger::{
    BatchProcessEntry, BlockError, DEFAULT_OPTIMISTIC_RETRIES, Ledger, WriterType,
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

    pub fn process_blocks(&mut self, mut batch: VecDeque<Arc<BlockContext>>, dequeue_wait_ns: u64) {
        let now = self.clock.now();
        let process_start = Instant::now();

        self.roll_back_competitor_blocks(&batch);

        let _concurrency_guard = self.stats.start_optimistic_writer();
        let prev_optimistic_successes = self.ledger.optimistic_successes();
        let prev_optimistic_conflicts = self.ledger.optimistic_conflicts();
        let prev_pessimistic_fallbacks = self.ledger.pessimistic_fallbacks();

        let validation_start = Instant::now();
        let validation_results = self.ledger.validate_batch(batch.iter().map(|c| &c.block));
        let validation_ns = validation_start.elapsed().as_nanos() as u64;

        let txn_start = Instant::now();
        let mut apply_ns = 0;
        let processed_entries = self
            .ledger
            .tx_optimistic_process(
                WriterType::BlockProcessor,
                DEFAULT_OPTIMISTIC_RETRIES,
                |txn, deferred| {
                    let apply_start = Instant::now();
                    let (processed, inserted_hashes) =
                        self.ledger
                            .apply_validated_batch(txn, deferred, &validation_results);
                    apply_ns += apply_start.elapsed().as_nanos() as u64;
                    Ok((processed, inserted_hashes))
                },
            )
            .unwrap_or_else(|e| panic!("failed to process block batch: {e}"));
        let txn_ns = txn_start.elapsed().as_nanos() as u64;
        let optimistic_successes = self.ledger.optimistic_successes();
        let optimistic_conflicts = self.ledger.optimistic_conflicts();
        let pessimistic_fallbacks = self.ledger.pessimistic_fallbacks();

        self.stats.optimistic_successes.fetch_add(
            optimistic_successes.saturating_sub(prev_optimistic_successes),
            Relaxed,
        );
        self.stats.optimistic_conflicts.fetch_add(
            optimistic_conflicts.saturating_sub(prev_optimistic_conflicts),
            Relaxed,
        );
        self.stats.pessimistic_fallbacks.fetch_add(
            pessimistic_fallbacks.saturating_sub(prev_pessimistic_fallbacks),
            Relaxed,
        );
        let process_ns = process_start.elapsed().as_nanos() as u64;
        self.stats.record_batch(
            batch.len() as u64,
            dequeue_wait_ns,
            txn_ns,
            validation_ns,
            apply_ns,
            process_ns,
        );

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
    optimistic_active: AtomicU64,
    max_optimistic_concurrency: AtomicU64,
    batch_count: AtomicU64,
    batch_blocks: AtomicU64,
    total_dequeue_wait_ns: AtomicU64,
    total_txn_ns: AtomicU64,
    max_batch_size: AtomicU64,
    max_txn_ns: AtomicU64,
    total_validation_ns: AtomicU64,
    total_apply_ns: AtomicU64,
    total_process_ns: AtomicU64,
    max_validation_ns: AtomicU64,
    max_apply_ns: AtomicU64,
    max_process_ns: AtomicU64,
    max_dequeue_wait_ns: AtomicU64,
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
        result.insert(
            "block_processor_writer",
            "optimistic_max_concurrency",
            self.max_optimistic_concurrency.load(Relaxed),
        );

        result.insert(
            "block_processor_batch",
            "count",
            self.batch_count.load(Relaxed),
        );
        result.insert(
            "block_processor_batch",
            "blocks",
            self.batch_blocks.load(Relaxed),
        );
        result.insert(
            "block_processor_batch",
            "dequeue_wait_ns",
            self.total_dequeue_wait_ns.load(Relaxed),
        );
        result.insert(
            "block_processor_batch",
            "txn_ns",
            self.total_txn_ns.load(Relaxed),
        );
        result.insert(
            "block_processor_batch",
            "max_txn_ns",
            self.max_txn_ns.load(Relaxed),
        );
        result.insert(
            "block_processor_batch",
            "max_size",
            self.max_batch_size.load(Relaxed),
        );
        result.insert(
            "block_processor_batch",
            "dequeue_wait_max_ns",
            self.max_dequeue_wait_ns.load(Relaxed),
        );
        result.insert(
            "block_processor_batch",
            "validate_ns",
            self.total_validation_ns.load(Relaxed),
        );
        result.insert(
            "block_processor_batch",
            "apply_ns",
            self.total_apply_ns.load(Relaxed),
        );
        result.insert(
            "block_processor_batch",
            "process_ns",
            self.total_process_ns.load(Relaxed),
        );
        result.insert(
            "block_processor_batch",
            "max_validate_ns",
            self.max_validation_ns.load(Relaxed),
        );
        result.insert(
            "block_processor_batch",
            "max_apply_ns",
            self.max_apply_ns.load(Relaxed),
        );
        result.insert(
            "block_processor_batch",
            "max_process_ns",
            self.max_process_ns.load(Relaxed),
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

    pub fn max_optimistic_concurrency(&self) -> u64 {
        self.max_optimistic_concurrency.load(Relaxed)
    }

    fn start_optimistic_writer(&self) -> OptimisticWriterGuard<'_> {
        let active = self.optimistic_active.fetch_add(1, Relaxed) + 1;
        self.update_max_concurrency(active);
        OptimisticWriterGuard { stats: self }
    }

    fn record_batch(
        &self,
        batch_size: u64,
        dequeue_wait_ns: u64,
        txn_ns: u64,
        validation_ns: u64,
        apply_ns: u64,
        process_ns: u64,
    ) {
        self.batch_count.fetch_add(1, Relaxed);
        self.batch_blocks.fetch_add(batch_size, Relaxed);
        self.total_dequeue_wait_ns
            .fetch_add(dequeue_wait_ns, Relaxed);
        self.total_txn_ns.fetch_add(txn_ns, Relaxed);
        self.total_validation_ns.fetch_add(validation_ns, Relaxed);
        self.total_apply_ns.fetch_add(apply_ns, Relaxed);
        self.total_process_ns.fetch_add(process_ns, Relaxed);

        self.update_max(&self.max_batch_size, batch_size);
        self.update_max(&self.max_txn_ns, txn_ns);
        self.update_max(&self.max_validation_ns, validation_ns);
        self.update_max(&self.max_apply_ns, apply_ns);
        self.update_max(&self.max_process_ns, process_ns);
        self.update_max(&self.max_dequeue_wait_ns, dequeue_wait_ns);
    }

    fn update_max_concurrency(&self, current: u64) {
        let mut observed = self.max_optimistic_concurrency.load(Relaxed);
        while current > observed {
            match self
                .max_optimistic_concurrency
                .compare_exchange(observed, current, Relaxed, Relaxed)
            {
                Ok(_) => break,
                Err(actual) => observed = actual,
            }
        }
    }

    fn end_optimistic_writer(&self) {
        self.optimistic_active.fetch_sub(1, Relaxed);
    }

    fn update_max(&self, target: &AtomicU64, candidate: u64) {
        let mut observed = target.load(Relaxed);
        while candidate > observed {
            match target.compare_exchange(observed, candidate, Relaxed, Relaxed) {
                Ok(_) => break,
                Err(actual) => observed = actual,
            }
        }
    }
}

struct OptimisticWriterGuard<'a> {
    stats: &'a BlockBatchProcessorStats,
}

impl Drop for OptimisticWriterGuard<'_> {
    fn drop(&mut self) {
        self.stats.end_optimistic_writer();
    }
}
