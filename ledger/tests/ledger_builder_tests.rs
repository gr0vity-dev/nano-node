use rsnano_ledger::{LedgerBuilder, LedgerConstants};
use rsnano_store_lmdb::LmdbLedgerStoreFactory;
use store_rocksdb::RocksdbLedgerStoreFactory;
use store_traits::config::{LedgerBackend, LedgerStoreConfig, RocksDbConfig};

#[test]
fn finish_uses_lmdb_factory() {
    let dir = tempfile::tempdir().unwrap();
    let factory = LmdbLedgerStoreFactory::default();
    let ledger = LedgerBuilder::new(dir.path().join("ledger.ldb"), &factory)
        .constants(LedgerConstants::unit_test())
        .finish()
        .expect("ledger builder should work with LMDB factory");

    assert!(ledger.account_count() >= 1);
}

#[test]
fn finish_uses_rocksdb_factory_when_configured() {
    let dir = tempfile::tempdir().unwrap();
    let factory = RocksdbLedgerStoreFactory::default();
    let ledger = LedgerBuilder::new(dir.path().join("rocksdb-ledger"), &factory)
        .store_config(LedgerStoreConfig::new(LedgerBackend::RocksDb(
            RocksDbConfig::default(),
        )))
        .constants(LedgerConstants::unit_test())
        .finish()
        .expect("ledger builder should support RocksDB backend");

    assert!(ledger.account_count() >= 1);
}
