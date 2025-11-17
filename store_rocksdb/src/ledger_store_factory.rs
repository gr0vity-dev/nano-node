use std::{path::PathBuf, sync::Arc};

use anyhow::bail;
use store_traits::ledger::{LedgerCache, LedgerStore, LedgerStoreFactory};
use store_traits::types::StoreEnvironmentFlags;
use store_traits::{
    config::{LedgerBackend, LedgerStoreConfig},
    environment::StoreEnvironmentFactory,
};

use crate::{
    environment::{RocksdbStoreEnvironment, RocksdbStoreEnvironmentFactory},
    ledger_impl::RocksdbLedgerStore,
};

pub struct RocksdbLedgerStoreFactory;

impl Default for RocksdbLedgerStoreFactory {
    fn default() -> Self {
        Self
    }
}

impl RocksdbLedgerStoreFactory {
    pub fn new() -> Self {
        Self
    }
}

impl LedgerStoreFactory for RocksdbLedgerStoreFactory {
    fn create_store(
        &self,
        path: PathBuf,
        config: LedgerStoreConfig,
        cache: Arc<LedgerCache>,
    ) -> anyhow::Result<Arc<dyn LedgerStore>> {
        let rocks_config = match config.backend {
            LedgerBackend::RocksDb(cfg) => cfg,
            _ => bail!("RocksDB factory requires RocksDB backend config"),
        };
        let env = RocksdbStoreEnvironment::open(
            path,
            StoreEnvironmentFlags::empty(),
            None,
            Some(&rocks_config),
        )?;
        RocksdbLedgerStore::create(Arc::new(env), cache)
    }

    fn create_null_store(&self, cache: Arc<LedgerCache>) -> anyhow::Result<Arc<dyn LedgerStore>> {
        let env_factory = RocksdbStoreEnvironmentFactory::default();
        let env = env_factory.create_null();
        RocksdbLedgerStore::create(env, cache)
    }
}
