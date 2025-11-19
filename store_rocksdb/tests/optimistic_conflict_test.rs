use std::sync::Arc;

use rsnano_types::SavedBlock;
use store_rocksdb::{RocksdbBlockStore, RocksdbLedgerWriteTxn, RocksdbStoreEnvironment};
use store_traits::{
    config::RocksDbConfig,
    transaction::LedgerWriteTxn,
    types::{StoreEnvironmentFlags, StoreErrorKind},
};

#[test]
fn duplicate_block_commit_returns_conflict() {
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

    let mut txn1 = RocksdbLedgerWriteTxn::new(&env);
    block_store.put(&mut txn1, &block);

    let mut txn2 = RocksdbLedgerWriteTxn::new(&env);
    block_store.put(&mut txn2, &block);

    Box::new(txn1).commit().unwrap();
    let err = Box::new(txn2)
        .commit()
        .expect_err("second commit should report conflict");
    assert_eq!(err.kind(), StoreErrorKind::Conflict);
}
