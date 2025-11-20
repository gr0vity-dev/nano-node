use std::sync::{Arc, Barrier};

use rsnano_ledger::Ledger;
use rsnano_types::{BlockHash, QualifiedRoot};
use store_rocksdb::default_ledger_store_factory;
use store_traits::ledger::WriterType;

#[test]
fn record_final_vote_uses_optimistic_transactions() {
    let ledger = Ledger::new_null(default_ledger_store_factory());
    let stats = ledger.final_vote_writer_stats();
    let root = QualifiedRoot::new_test_instance();
    let hash = BlockHash::from(1234);

    ledger.record_final_vote(&root, &hash).unwrap();

    let txn = ledger.store.begin_read();
    assert_eq!(
        ledger.store.final_vote().get(txn.as_ref(), &root),
        Some(hash)
    );
    assert_eq!(stats.optimistic_successes(), 1);
    assert_eq!(stats.optimistic_conflicts(), 0);
    assert_eq!(stats.pessimistic_fallbacks(), 0);
    assert!(stats.max_optimistic_concurrency() >= 1);
}

#[test]
fn final_vote_writes_can_run_concurrently_with_other_writers() {
    let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));
    let stats = ledger.final_vote_writer_stats();
    let root = QualifiedRoot::new_test_instance();
    let hash = BlockHash::from(9999);
    let barrier = Arc::new(Barrier::new(2));

    let ledger_clone = Arc::clone(&ledger);
    let barrier_clone = Arc::clone(&barrier);
    let block_writer = std::thread::spawn(move || {
        ledger_clone
            .tx_optimistic_process(WriterType::BlockProcessor, 0, |_txn, _| {
                barrier_clone.wait();
                Ok(((), Vec::new()))
            })
            .unwrap();
    });

    ledger
        .apply_final_vote_op(|txn| {
            barrier.wait();
            ledger.store.final_vote().put(txn, &root, &hash);
        })
        .unwrap();

    block_writer.join().unwrap();

    let txn = ledger.store.begin_read();
    assert_eq!(
        ledger.store.final_vote().get(txn.as_ref(), &root),
        Some(hash)
    );
    assert!(stats.optimistic_successes() >= 1);
    assert_eq!(stats.optimistic_conflicts(), 0);
    assert_eq!(stats.pessimistic_fallbacks(), 0);
}
