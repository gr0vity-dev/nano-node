use std::{collections::VecDeque, sync::Arc};

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
