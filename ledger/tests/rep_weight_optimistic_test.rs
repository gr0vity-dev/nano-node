use std::sync::{Arc, Barrier};

use rsnano_ledger::{DeferredLedgerOperations, Ledger, RepWeightWriterStats};
use rsnano_types::{Amount, PublicKey};
use store_rocksdb::default_ledger_store_factory;
use store_traits::ledger::WriterType;

#[test]
fn deferred_rep_weight_updates_use_optimistic_transactions() {
    let ledger = Ledger::new_null(default_ledger_store_factory());
    let stats = ledger.rep_weight_writer_stats();
    let rep = PublicKey::from(42);

    let mut deferred = DeferredLedgerOperations::new();
    deferred.add_rep_weight(rep, Amount::raw(5));
    deferred.execute(&ledger);

    assert_eq!(ledger.rep_weights.weight(&rep), Amount::raw(5));
    assert_eq!(stats.optimistic_successes(), 1);
    assert_eq!(stats.optimistic_conflicts(), 0);
    assert_eq!(stats.pessimistic_fallbacks(), 0);
    assert!(stats.max_optimistic_concurrency() >= 1);
}

#[test]
fn rep_weight_updates_can_run_concurrently_with_block_writer() {
    let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));
    let stats: Arc<RepWeightWriterStats> = ledger.rep_weight_writer_stats();
    let rep = PublicKey::from(7);
    let barrier = Arc::new(Barrier::new(2));

    let ledger_clone = ledger.clone();
    let barrier_clone = barrier.clone();
    let block_handle = std::thread::spawn(move || {
        ledger_clone
            .tx_optimistic_process(WriterType::BlockProcessor, 0, |_txn, _| {
                barrier_clone.wait();
                Ok(((), Vec::new()))
            })
            .unwrap();
    });

    ledger.apply_rep_weight_ops(|txn| {
        ledger
            .rep_weights_updater
            .representation_add(txn, rep, Amount::raw(9));
        barrier.wait();
    });

    block_handle.join().unwrap();

    assert_eq!(ledger.rep_weights.weight(&rep), Amount::raw(9));
    assert!(stats.optimistic_successes() >= 1);
    assert_eq!(stats.optimistic_conflicts(), 0);
    assert_eq!(stats.pessimistic_fallbacks(), 0);
}
