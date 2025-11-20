use std::{collections::VecDeque, sync::Arc};

use rsnano_ledger::WriterType;
use rsnano_node::block_processing::{BlockBatchProcessor, BlockContext};

#[test]
fn block_batch_processor_uses_optimistic_transactions() {
    let mut processor = BlockBatchProcessor::new_null();
    let batch: VecDeque<_> = VecDeque::new();

    processor.process_blocks(batch);

    assert_eq!(processor.ledger.optimistic_successes(), 1);
    assert_eq!(processor.ledger.optimistic_conflicts(), 0);
    assert_eq!(processor.ledger.pessimistic_fallbacks(), 0);
    assert_eq!(processor.stats.optimistic_successes(), 1);
    assert_eq!(processor.stats.optimistic_conflicts(), 0);
    assert_eq!(processor.stats.pessimistic_fallbacks(), 0);

    // Sanity: processing a real block still routes through optimistic path
    let mut batch = VecDeque::new();
    batch.push_back(Arc::new(BlockContext::new_test_instance()));
    processor.process_blocks(batch);
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

    processor.process_blocks(VecDeque::new());

    // Ledger tracks both writers; block processor stats should only count its own attempt
    assert_eq!(processor.ledger.optimistic_successes(), 2);
    assert_eq!(processor.stats.optimistic_successes(), 1);
    assert_eq!(processor.stats.optimistic_conflicts(), 0);
    assert_eq!(processor.stats.pessimistic_fallbacks(), 0);
}
