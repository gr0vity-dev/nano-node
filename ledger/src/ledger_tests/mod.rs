macro_rules! ledger_backend_tests {
    ($mod_name:ident, $factory:expr) => {
        mod $mod_name {
            use std::sync::Arc;
            use store_traits::ledger::LedgerStoreFactory;

            fn backend_factory() -> Arc<dyn LedgerStoreFactory> {
                $factory
            }

            include!("backend_tests.rs");
        }
    };
}

ledger_backend_tests!(
    rocksdb,
    std::sync::Arc::new(store_rocksdb::RocksdbLedgerStoreFactory::default())
);
