use std::{path::PathBuf, sync::Arc};

use rsnano_nullable_lmdb::{
    ConfiguredDatabase, EnvironmentOptions, LmdbEnvironment, LmdbEnvironmentFactory,
};
use store_traits::{
    config::LedgerStoreConfig,
    ledger::{LedgerCache, LedgerStore, LedgerStoreFactory},
};

use crate::{LmdbStore, create_and_update_lmdb_env, get_lmdb_flags};

pub struct LmdbLedgerStoreFactory {
    env_factory: LmdbEnvironmentFactory,
}

impl Default for LmdbLedgerStoreFactory {
    fn default() -> Self {
        Self::new(LmdbEnvironmentFactory::default())
    }
}

impl LmdbLedgerStoreFactory {
    pub fn new(env_factory: LmdbEnvironmentFactory) -> Self {
        Self { env_factory }
    }

    pub fn new_null() -> Self {
        Self::new(LmdbEnvironmentFactory::new_null())
    }

    fn env_options(path: PathBuf, config: &LedgerStoreConfig) -> EnvironmentOptions {
        EnvironmentOptions {
            max_dbs: config.max_databases,
            map_size: config.map_size,
            flags: get_lmdb_flags(config),
            path,
        }
    }

    fn build_store(
        &self,
        env: LmdbEnvironment,
        cache: Arc<LedgerCache>,
    ) -> anyhow::Result<Arc<dyn LedgerStore>> {
        let mut store_impl = LmdbStore::new(env)?;
        store_impl.cache = cache;
        Ok(Arc::new(store_impl))
    }
}

impl LedgerStoreFactory for LmdbLedgerStoreFactory {
    fn create_store(
        &self,
        path: PathBuf,
        config: LedgerStoreConfig,
        cache: Arc<LedgerCache>,
    ) -> anyhow::Result<Arc<dyn LedgerStore>> {
        let env_options = Self::env_options(path, &config);
        let env = create_and_update_lmdb_env(&self.env_factory, env_options)?;
        self.build_store(env, cache)
    }

    fn create_null_store(&self, cache: Arc<LedgerCache>) -> anyhow::Result<Arc<dyn LedgerStore>> {
        let env = LmdbEnvironment::new_null();
        self.build_store(env, cache)
    }
}

pub fn create_null_store_with_databases(
    databases: Vec<ConfiguredDatabase>,
    cache: Arc<LedgerCache>,
) -> anyhow::Result<Arc<dyn LedgerStore>> {
    let mut builder = LmdbEnvironment::null_builder();
    for database in databases {
        builder = builder.configured_database(database);
    }
    let env = builder.build();
    let mut store_impl = LmdbStore::new(env)?;
    store_impl.cache = cache;
    Ok(Arc::new(store_impl))
}
