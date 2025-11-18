use rsnano_ledger::{AnySet, LedgerBuilder, LedgerConstants, LedgerInserter};
use rsnano_types::PrivateKey;
use store_rocksdb::RocksdbLedgerStoreFactory;
use store_traits::config::{LedgerBackend, LedgerStoreConfig, RocksDbConfig};

#[test]
fn rocksdb_ledger_subset_basic_flow() {
    let dir = tempfile::tempdir().unwrap();
    let factory = RocksdbLedgerStoreFactory::default();
    let ledger_path = dir.path().join("ledger");
    let ledger = LedgerBuilder::new(ledger_path, &factory)
        .config(LedgerStoreConfig::new(LedgerBackend::RocksDb(
            RocksDbConfig::default(),
        )))
        .constants(LedgerConstants::unit_test())
        .finish()
        .expect("ledger should build with RocksDB backend");

    let inserter = LedgerInserter::new(&ledger);
    let receiver_key = PrivateKey::from(1);
    let send = inserter.genesis().send(receiver_key.account(), 1000);

    assert_eq!(
        ledger
            .any()
            .block_successor_by_qualified_root(&ledger.genesis().qualified_root()),
        Some(ledger.genesis().hash())
    );
    assert_eq!(
        ledger
            .any()
            .block_successor_by_qualified_root(&send.qualified_root()),
        Some(send.hash())
    );

    let receive = inserter.account(&receiver_key).receive(send.hash());
    assert_eq!(
        ledger.any().block_account(&receive.hash()),
        Some(receiver_key.account())
    );
}
