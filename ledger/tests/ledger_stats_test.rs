use std::{
    sync::{
        Arc,
        mpsc::{self},
    },
    thread,
    time::Duration,
};

use rsnano_ledger::{
    CommitDisposition, DeferredLedgerOperations, Ledger, block_insertion::BlockInserter,
};
use rsnano_utils::stats::{Direction, StatsCollection, StatsSource};
use store_rocksdb::default_ledger_store_factory;
use store_traits::ledger::{WriteStrategy, WriterType};
use store_traits::types::StoreErrorKind;

mod insertion_test_helpers {
    use rsnano_ledger as ledger_crate;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/common/test_helpers.rs"
    ));
}
use insertion_test_helpers::{commit_block_txn, legacy_open_block_instructions};

fn new_ledger() -> Ledger {
    Ledger::new_null(default_ledger_store_factory())
}

#[test]
fn ledger_stats_collect_conflict_hotspots() {
    let ledger = Arc::new(new_ledger());
    let (block, instructions) = legacy_open_block_instructions();

    let (ready_tx, ready_rx) = mpsc::channel();
    let (start_tx, start_rx) = mpsc::channel::<()>();
    let (committed_tx, committed_rx) = mpsc::channel();
    let ledger_clone = Arc::clone(&ledger);
    let mut thread_block = block.clone();
    let thread_instructions = instructions.clone();
    let writer = WriterType::Testing;
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker = thread::spawn(move || {
        ready_tx.send(()).unwrap();
        start_rx.recv().unwrap();
        let mut txn = ledger_clone.begin_write_with(writer, WriteStrategy::Optimistic);
        let mut deferred = DeferredLedgerOperations::new();
        let (saved, inserted, _) = BlockInserter::new(
            &ledger_clone,
            txn.as_mut(),
            &mut thread_block,
            &thread_instructions,
        )
        .insert(&mut deferred);
        let disposition = commit_block_txn(&ledger_clone, txn, inserted, saved.as_ref());
        if matches!(disposition, CommitDisposition::Success) && inserted {
            deferred.execute(&ledger_clone);
        }
        committed_tx.send(()).unwrap();
    });

    ready_rx.recv().unwrap();

    ledger
        .tx_optimistic_process(writer, 0, |txn, deferred| {
            let mut block_local = block.clone();
            let instructions_local = instructions.clone();
            let (saved, inserted, _) =
                BlockInserter::new(&ledger, txn, &mut block_local, &instructions_local)
                    .insert(deferred);
            // Start the worker only after this optimistic writer has begun to ensure it will
            // observe a concurrent commit and record the conflict.
            if !started.swap(true, std::sync::atomic::Ordering::SeqCst) {
                start_tx.send(()).unwrap();
                committed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            }
            let hashes = saved
                .iter()
                .filter(|_| inserted)
                .map(|b| b.hash())
                .collect();
            Ok(((), hashes))
        })
        .map_err(|e| match e.kind() {
            StoreErrorKind::Conflict => e,
            other => panic!("unexpected error {other:?}"),
        })
        .expect("fallback path should succeed after conflict");

    worker.join().unwrap();

    let mut collected = StatsCollection::new();
    ledger.collect_stats(&mut collected);

    assert!(
        collected.get("ledger_writer_conflicts", writer.as_str()) >= 1,
        "writer-specific conflict counter should increment"
    );
    assert!(
        collected.get("ledger_writer_fallbacks", writer.as_str()) >= 1,
        "writer-specific fallback counter should increment"
    );
    assert!(
        collected.get("ledger_writer", "optimistic_conflicts") >= 1,
        "global conflict counter should increment"
    );
}

#[test]
fn ledger_stats_report_write_queue_depth() {
    let ledger = Arc::new(new_ledger());
    let pessimistic_txn = ledger.begin_write_with(WriterType::Testing, WriteStrategy::Pessimistic);

    let ledger_clone = Arc::clone(&ledger);
    let waiter = thread::spawn(move || {
        let _txn = ledger_clone.begin_write_with(WriterType::Testing, WriteStrategy::Optimistic);
    });

    assert!(
        ledger.wait_for_write_queue_waiting_optimistic(1, Duration::from_secs(2)),
        "optimistic writer should appear in queue"
    );

    let mut collected = StatsCollection::new();
    ledger.collect_stats(&mut collected);
    assert_eq!(collected.get("ledger_write_queue", "queue_depth"), 1);

    drop(pessimistic_txn);
    waiter.join().unwrap();
}

#[test]
fn ledger_write_queue_stats_present() {
    let ledger = Arc::new(new_ledger());

    let mut collected = StatsCollection::new();
    ledger.collect_stats(&mut collected);

    assert!(
        collected.contains("ledger_write_queue", "queue_depth", Direction::In),
        "ledger_write_queue queue_depth stat should be reported"
    );
    assert!(
        collected.contains("ledger_write_queue", "optimistic_active", Direction::In),
        "ledger_write_queue optimistic_active stat should be reported"
    );
}
