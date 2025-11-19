use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use rsnano_types::SavedBlock;
use store_rocksdb::{RocksdbBlockStore, RocksdbLedgerWriteTxn, RocksdbStoreEnvironment};
use store_traits::{
    config::RocksDbConfig,
    transaction::LedgerWriteTxn,
    types::{StoreEnvironmentFlags, StoreErrorKind},
};

#[test]
fn commit_callbacks_fire_only_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let env = Arc::new(
        RocksdbStoreEnvironment::open(
            dir.path().to_path_buf(),
            StoreEnvironmentFlags::empty(),
            Some(dir),
            Some(&RocksDbConfig::default()),
        )
        .unwrap(),
    );

    let block_store = RocksdbBlockStore::new(Arc::clone(&env)).unwrap();
    let block = SavedBlock::new_test_instance();
    let callbacks = Arc::new(AtomicUsize::new(0));

    let mut txn1 = RocksdbLedgerWriteTxn::new(&env);
    {
        let counter = Arc::clone(&callbacks);
        txn1.on_commit(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
    }
    block_store.put(&mut txn1, &block);

    let mut txn2 = RocksdbLedgerWriteTxn::new(&env);
    {
        let counter = Arc::clone(&callbacks);
        txn2.on_commit(Box::new(move || {
            counter.fetch_add(10, Ordering::SeqCst);
        }));
    }
    block_store.put(&mut txn2, &block);

    Box::new(txn1).commit().unwrap();
    assert_eq!(callbacks.load(Ordering::SeqCst), 1);

    let err = Box::new(txn2)
        .commit()
        .expect_err("second commit should conflict");
    assert_eq!(err.kind(), StoreErrorKind::Conflict);
    assert_eq!(callbacks.load(Ordering::SeqCst), 1);
}
