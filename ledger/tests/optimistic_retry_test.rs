use rsnano_ledger::{Ledger, block_insertion::BlockInserter};
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
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};
use store_rocksdb::default_ledger_store_factory;
use store_traits::ledger::{LedgerStoreFactory, WriteStrategy, WriterType};

#[test]
fn optimistic_retry_recovers_after_conflict() {
    let ledger = Arc::new(new_ledger());
    let (block, instructions) = legacy_open_block_instructions();

    let (signal_tx, signal_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let ledger_clone = Arc::<Ledger>::clone(&ledger);
    let mut thread_block = block.clone();
    let thread_instructions = instructions.clone();
    let handle = thread::spawn(move || {
        signal_rx.recv().unwrap();
        let mut txn = ledger_clone.begin_write_with(WriterType::Testing, WriteStrategy::Optimistic);
        let (saved, inserted, _) = BlockInserter::new(
            &ledger_clone,
            txn.as_mut(),
            &mut thread_block,
            &thread_instructions,
        )
        .insert();
        assert!(inserted);
        commit_block_txn(&ledger_clone, txn, inserted, saved.as_ref());
        done_tx.send(()).unwrap();
    });

    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_clone = Arc::clone(&attempts);
    ledger
        .tx_optimistic_process(WriterType::Testing, 1, |txn| {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            let mut block_local = block.clone();
            let instructions_local = instructions.clone();
            let (saved, inserted, _) =
                BlockInserter::new(&ledger, txn, &mut block_local, &instructions_local).insert();
            signal_tx.send(()).unwrap();
            done_rx.recv().unwrap();
            let hashes = saved
                .iter()
                .filter(|_| inserted)
                .map(|b| b.hash())
                .collect();
            Ok(((), hashes))
        })
        .expect("optimistic retry should succeed");

    handle.join().unwrap();

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(ledger.optimistic_conflicts(), 1);
    assert_eq!(ledger.optimistic_successes(), 1);
    assert_eq!(ledger.pessimistic_fallbacks(), 0);
}

#[test]
fn pessimistic_fallback_after_conflict() {
    let ledger = Arc::new(new_ledger());
    let (block, instructions) = legacy_open_block_instructions();

    let (signal_tx, signal_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let ledger_clone = Arc::<Ledger>::clone(&ledger);
    let mut thread_block = block.clone();
    let thread_instructions = instructions.clone();
    let handle = thread::spawn(move || {
        signal_rx.recv().unwrap();
        let mut txn = ledger_clone.begin_write_with(WriterType::Testing, WriteStrategy::Optimistic);
        let (saved, inserted, _) = BlockInserter::new(
            &ledger_clone,
            txn.as_mut(),
            &mut thread_block,
            &thread_instructions,
        )
        .insert();
        assert!(inserted);
        commit_block_txn(&ledger_clone, txn, inserted, saved.as_ref());
        done_tx.send(()).unwrap();
    });

    ledger
        .tx_optimistic_process(WriterType::Testing, 0, |txn| {
            let mut block_local = block.clone();
            let instructions_local = instructions.clone();
            let (saved, inserted, _) =
                BlockInserter::new(&ledger, txn, &mut block_local, &instructions_local).insert();
            signal_tx.send(()).unwrap();
            done_rx.recv().unwrap();
            let hashes = saved
                .iter()
                .filter(|_| inserted)
                .map(|b| b.hash())
                .collect();
            Ok(((), hashes))
        })
        .expect("fallback should succeed");

    handle.join().unwrap();

    assert_eq!(ledger.optimistic_conflicts(), 1);
    assert_eq!(ledger.optimistic_successes(), 0);
    assert_eq!(ledger.pessimistic_fallbacks(), 1);
}

fn new_ledger() -> Ledger {
    Ledger::new_null(test_store_factory())
}

fn test_store_factory() -> Arc<dyn LedgerStoreFactory> {
    default_ledger_store_factory()
}
