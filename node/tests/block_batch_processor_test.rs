use std::{collections::VecDeque, sync::Arc};

use rsnano_ledger::WriterType;
use rsnano_node::block_processing::{BlockBatchProcessor, BlockContext};
use rsnano_utils::stats::{Direction, StatsCollection, StatsSource};

#[test]
fn block_batch_processor_uses_optimistic_transactions() {
    let mut processor = BlockBatchProcessor::new_null();
    let batch: VecDeque<_> = VecDeque::new();

    processor.process_blocks(batch, 0);

    assert_eq!(processor.ledger.optimistic_successes(), 1);
    assert_eq!(processor.ledger.optimistic_conflicts(), 0);
    assert_eq!(processor.ledger.pessimistic_fallbacks(), 0);
    assert_eq!(processor.stats.optimistic_successes(), 1);
    assert_eq!(processor.stats.optimistic_conflicts(), 0);
    assert_eq!(processor.stats.pessimistic_fallbacks(), 0);
    assert_eq!(processor.stats.max_optimistic_concurrency(), 1);

    // Sanity: processing a real block still routes through optimistic path
    let mut batch = VecDeque::new();
    batch.push_back(Arc::new(BlockContext::new_test_instance()));
    processor.process_blocks(batch, 0);
    assert!(processor.ledger.optimistic_successes() >= 2);
}

#[test]
fn writer_stats_ignore_external_ledger_activity() {
    let mut processor = BlockBatchProcessor::new_null();

    processor
        .ledger
        .tx_optimistic_process(WriterType::Testing, 0, |_, _| Ok(((), Vec::new())))
        .expect("pre-flight ledger transaction should succeed");

    assert_eq!(processor.stats.optimistic_successes(), 0);

    processor.process_blocks(VecDeque::new(), 0);

    // Ledger tracks both writers; block processor stats should only count its own attempt
    assert_eq!(processor.ledger.optimistic_successes(), 2);
    assert_eq!(processor.stats.optimistic_successes(), 1);
    assert_eq!(processor.stats.optimistic_conflicts(), 0);
    assert_eq!(processor.stats.pessimistic_fallbacks(), 0);
    assert!(processor.stats.max_optimistic_concurrency() >= 1);
}

#[test]
fn records_batch_metrics() {
    let mut processor = BlockBatchProcessor::new_null();

    let mut first_batch = VecDeque::new();
    first_batch.push_back(Arc::new(BlockContext::new_test_instance()));
    processor.process_blocks(first_batch, 5);

    let mut second_batch = VecDeque::new();
    second_batch.push_back(Arc::new(BlockContext::new_test_instance()));
    second_batch.push_back(Arc::new(BlockContext::new_test_instance()));
    processor.process_blocks(second_batch, 7);

    let mut stats = StatsCollection::new();
    processor.stats.collect_stats(&mut stats);

    assert_eq!(stats.get("block_processor_batch", "count"), 2);
    assert_eq!(stats.get("block_processor_batch", "blocks"), 3);
    assert_eq!(stats.get("block_processor_batch", "dequeue_wait_ns"), 12);
    assert_eq!(stats.get("block_processor_batch", "max_size"), 2);
    assert!(stats.contains("block_processor_batch", "validate_ns", Direction::In));
    assert!(stats.contains("block_processor_batch", "apply_ns", Direction::In));
    assert!(stats.contains("block_processor_batch", "process_ns", Direction::In));
}
