use rsnano_ledger::{Ledger, block_insertion::BlockInserter};
mod insertion_test_helpers {
    use rsnano_ledger as ledger_crate;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/common/test_helpers.rs"
    ));
}
use insertion_test_helpers::legacy_open_block_instructions;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use store_rocksdb::default_ledger_store_factory;
use store_traits::ledger::{LedgerStoreFactory, WriterType};

#[test]
fn rep_weights_run_after_commit() {
    let ledger = Ledger::new_null(test_store_factory());
    let rep_weight_puts = ledger.store.rep_weight().track_puts();
    let commit_seen = Arc::new(AtomicBool::new(false));
    let deferred_seen = Arc::new(AtomicBool::new(false));

    let (block, instructions) = legacy_open_block_instructions();
    let expected_rep = instructions.set_account_info.representative;
    let expected_weight = instructions.set_account_info.balance;

    ledger
        .tx_optimistic_process(WriterType::Testing, 1, |txn, deferred| {
            let commit_seen = Arc::clone(&commit_seen);
            let rep_weight_puts = Arc::clone(&rep_weight_puts);
            let deferred_seen = Arc::clone(&deferred_seen);
            txn.on_commit(Box::new(move || {
                assert!(
                    rep_weight_puts.output().is_empty(),
                    "rep weights should not be updated before commit"
                );
                commit_seen.store(true, Ordering::SeqCst);
            }));

            let mut block_local = block.clone();
            let instructions_local = instructions.clone();
            let (saved, inserted, _) =
                BlockInserter::new(&ledger, txn, &mut block_local, &instructions_local)
                    .insert(deferred);
            deferred.add_custom(move |_| {
                deferred_seen.store(true, Ordering::SeqCst);
            });
            let hashes = saved
                .iter()
                .filter(|_| inserted)
                .map(|b| b.hash())
                .collect();
            Ok(((), hashes))
        })
        .expect("optimistic process should succeed");

    assert!(
        commit_seen.load(Ordering::SeqCst),
        "on_commit should run before deferred work"
    );
    assert!(
        deferred_seen.load(Ordering::SeqCst),
        "deferred work should execute after commit"
    );
    assert_eq!(
        rep_weight_puts.output(),
        vec![(expected_rep, expected_weight)],
        "rep weight updates should run after commit"
    );
}

fn test_store_factory() -> Arc<dyn LedgerStoreFactory> {
    default_ledger_store_factory()
}
