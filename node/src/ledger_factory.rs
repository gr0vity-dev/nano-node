use std::sync::Arc;

use store_traits::ledger::LedgerStoreFactory;

/// Returns the default ledger store factory used by node tests when a specific
/// backend isn't injected. This currently points at RocksDB so nulled nodes
/// don't pull in LMDB-only helpers.
pub(crate) fn default_ledger_store_factory() -> Arc<dyn LedgerStoreFactory> {
    store_rocksdb::default_ledger_store_factory()
}
