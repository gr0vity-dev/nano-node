use rsnano_ledger::{
    CommitDisposition, DeferredLedgerOperations, Ledger, block_insertion::BlockInserter,
};
mod insertion_test_helpers {
    use rsnano_ledger as ledger_crate;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/common/test_helpers.rs"
    ));
}
use insertion_test_helpers::{commit_block_txn, legacy_open_block_instructions};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
use store_rocksdb::default_ledger_store_factory;
use store_traits::ledger::{LedgerStoreFactory, WriteStrategy, WriterType};
use store_traits::types::{StoreError, StoreErrorKind};

#[test]
fn optimistic_retry_recovers_after_conflict() {
    let ledger = Arc::new(new_ledger());
    let (block, instructions) = legacy_open_block_instructions();

    let (signal_tx, signal_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let ledger_clone = Arc::<Ledger>::clone(&ledger);
    let first_attempt = Arc::new(AtomicBool::new(true));
    let first_attempt_clone = Arc::clone(&first_attempt);
    let mut thread_block = block.clone();
    let thread_instructions = instructions.clone();
    let handle = thread::spawn(move || {
        signal_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("worker did not receive start signal in time");
        let mut txn = ledger_clone.begin_write_with(WriterType::Testing, WriteStrategy::Optimistic);
        let mut deferred = DeferredLedgerOperations::new();
        let (saved, inserted, _) = BlockInserter::new(
            &ledger_clone,
            txn.as_mut(),
            &mut thread_block,
            &thread_instructions,
        )
        .insert(&mut deferred);
        assert!(inserted);
        let disposition = commit_block_txn(&ledger_clone, txn, inserted, saved.as_ref());
        done_tx.send(()).unwrap();
        if inserted && matches!(disposition, CommitDisposition::Success) {
            deferred.execute(&ledger_clone);
        }
    });

    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_clone = Arc::clone(&attempts);
    let successes_before = ledger.optimistic_successes();
    let conflicts_before = ledger.optimistic_conflicts();
    let fallbacks_before = ledger.pessimistic_fallbacks();

    ledger
        .tx_optimistic_process(WriterType::Testing, 1, |txn, deferred| {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            let mut block_local = block.clone();
            let instructions_local = instructions.clone();
            let (saved, inserted, _) =
                BlockInserter::new(&ledger, txn, &mut block_local, &instructions_local)
                    .insert(deferred);
            if first_attempt_clone.swap(false, Ordering::SeqCst) {
                signal_tx
                    .send(())
                    .expect("failed to signal worker to proceed");
                done_rx
                    .recv_timeout(Duration::from_secs(2))
                    .expect("worker did not finish in time");
            }
            let hashes = saved
                .iter()
                .filter(|_| inserted)
                .map(|b| b.hash())
                .collect();
            Ok(((), hashes))
        })
        .expect("optimistic retry should succeed");

    handle.join().expect("worker thread panicked");

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(
        ledger.optimistic_conflicts() - conflicts_before >= 1,
        "expected at least one optimistic conflict"
    );
    assert!(
        ledger.optimistic_successes() - successes_before >= 1,
        "expected at least one optimistic success"
    );
    assert_eq!(ledger.pessimistic_fallbacks() - fallbacks_before, 0);
}

#[test]
fn pessimistic_fallback_after_conflict() {
    let ledger = Arc::new(new_ledger());
    let (block, instructions) = legacy_open_block_instructions();

    let (signal_tx, signal_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let ledger_clone = Arc::<Ledger>::clone(&ledger);
    let first_attempt = Arc::new(AtomicBool::new(true));
    let first_attempt_clone = Arc::clone(&first_attempt);
    let mut thread_block = block.clone();
    let thread_instructions = instructions.clone();
    let handle = thread::spawn(move || {
        signal_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("worker did not receive start signal in time");
        let mut txn = ledger_clone.begin_write_with(WriterType::Testing, WriteStrategy::Optimistic);
        let mut deferred = DeferredLedgerOperations::new();
        let (saved, inserted, _) = BlockInserter::new(
            &ledger_clone,
            txn.as_mut(),
            &mut thread_block,
            &thread_instructions,
        )
        .insert(&mut deferred);
        assert!(inserted);
        let disposition = commit_block_txn(&ledger_clone, txn, inserted, saved.as_ref());
        done_tx.send(()).unwrap();
        if inserted && matches!(disposition, CommitDisposition::Success) {
            deferred.execute(&ledger_clone);
        }
    });

    let successes_before = ledger.optimistic_successes();
    let conflicts_before = ledger.optimistic_conflicts();
    let fallbacks_before = ledger.pessimistic_fallbacks();

    ledger
        .tx_optimistic_process(WriterType::Testing, 0, |txn, deferred| {
            let mut block_local = block.clone();
            let instructions_local = instructions.clone();
            let (saved, inserted, _) =
                BlockInserter::new(&ledger, txn, &mut block_local, &instructions_local)
                    .insert(deferred);
            if first_attempt_clone.swap(false, Ordering::SeqCst) {
                signal_tx
                    .send(())
                    .expect("failed to signal worker to proceed");
                done_rx
                    .recv_timeout(Duration::from_secs(2))
                    .expect("worker did not finish in time");
            }
            let hashes = saved
                .iter()
                .filter(|_| inserted)
                .map(|b| b.hash())
                .collect();
            Ok(((), hashes))
        })
        .map_err(|e| match e.kind() {
            StoreErrorKind::Conflict => StoreError::new(StoreErrorKind::Conflict, "conflict"),
            other => StoreError::new(other, "tx_optimistic_process failed"),
        })
        .expect("fallback should succeed");

    handle.join().expect("worker thread panicked");

    assert!(
        ledger.optimistic_conflicts() - conflicts_before >= 1,
        "expected at least one optimistic conflict"
    );
    assert!(
        ledger.optimistic_successes() - successes_before >= 1,
        "worker should have succeeded optimistically even though caller fell back"
    );
    assert!(
        ledger.pessimistic_fallbacks() - fallbacks_before >= 1,
        "expected pessimistic fallback increment"
    );
}

fn new_ledger() -> Ledger {
    Ledger::new_null(test_store_factory())
}

fn test_store_factory() -> Arc<dyn LedgerStoreFactory> {
    default_ledger_store_factory()
}
