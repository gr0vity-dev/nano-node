use std::{path::PathBuf, sync::Arc};

use rsnano_nullable_lmdb::LmdbEnvironmentFactory;
use rsnano_store_lmdb::{
    EnvironmentOptions, LedgerCache, LmdbConfig, create_and_update_lmdb_env, get_lmdb_flags,
};
use rsnano_types::Amount;
use rsnano_utils::get_cpu_count;
use rsnano_utils::stats::Stats;

use crate::{BootstrapWeights, Ledger, LedgerConstants, LedgerEvent, RepWeightCache};

pub struct LedgerBuilder<'a> {
    path: PathBuf,
    config: Option<LmdbConfig>,
    env_factory: Option<&'a LmdbEnvironmentFactory>,
    bootstrap_weights: Option<BootstrapWeights>,
    stats: Option<Arc<Stats>>,
    min_rep_weight: Amount,
    ledger_constants: Option<LedgerConstants>,
    thread_count: usize,
    batch_validation_threads: Option<usize>,
    sender: Option<Box<dyn Fn(LedgerEvent) + Send + Sync>>,
    consistency_check: bool,
}

impl<'a> LedgerBuilder<'a> {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            config: None,
            env_factory: None,
            bootstrap_weights: None,
            stats: None,
            min_rep_weight: Amount::ZERO,
            ledger_constants: None,
            thread_count: 0,
            batch_validation_threads: None,
            sender: None,
            consistency_check: true,
        }
    }

    pub fn env_factory(mut self, env_factory: &'a LmdbEnvironmentFactory) -> Self {
        self.env_factory = Some(env_factory);
        self
    }

    pub fn config(mut self, config: LmdbConfig) -> Self {
        self.config = Some(config);
        self
    }

    pub fn bootstrap_weights(mut self, weights: BootstrapWeights) -> Self {
        self.bootstrap_weights = Some(weights);
        self
    }

    pub fn constants(mut self, constants: LedgerConstants) -> Self {
        self.ledger_constants = Some(constants);
        self
    }

    pub fn stats(mut self, stats: Arc<Stats>) -> Self {
        self.stats = Some(stats);
        self
    }

    pub fn min_rep_weight(mut self, weight: Amount) -> Self {
        self.min_rep_weight = weight;
        self
    }

    pub fn init_thread_count(mut self, count: usize) -> Self {
        self.thread_count = count;
        self
    }

    pub fn batch_validation_threads(mut self, count: usize) -> Self {
        self.batch_validation_threads = Some(count);
        self
    }

    pub fn publish_to<T>(mut self, tx: T) -> Self
    where
        T: Fn(LedgerEvent) + Send + Sync + 'static,
    {
        self.sender = Some(Box::new(tx));
        self
    }

    pub fn consistency_check(mut self, check: bool) -> Self {
        self.consistency_check = check;
        self
    }

    pub fn finish(mut self) -> anyhow::Result<Ledger> {
        let ledger_cache = Arc::new(LedgerCache::new());
        let bootstrap_weights = self.bootstrap_weights.unwrap_or_default();

        let rep_weights = Arc::new(RepWeightCache::with_bootstrap_weights(
            bootstrap_weights,
            ledger_cache.clone(),
            self.min_rep_weight,
        ));

        let config = self.config.unwrap_or_default();
        let default_env_factory = LmdbEnvironmentFactory::default();
        let env_factory = self.env_factory.unwrap_or(&default_env_factory);

        let env_options = EnvironmentOptions {
            max_dbs: config.max_databases,
            map_size: config.map_size,
            flags: get_lmdb_flags(&config),
            path: self.path,
        };

        let stats = self.stats.unwrap_or_else(|| Arc::new(Stats::default()));
        let ledger_constants = self.ledger_constants.unwrap_or_else(LedgerConstants::live);

        if self.thread_count == 0 {
            // Between 10 and 40 threads, scales well even in low power systems as long as actions are I/O bound
            self.thread_count = (11 * get_cpu_count()).clamp(10, 40);
        }
        let batch_validation_threads = self
            .batch_validation_threads
            .unwrap_or_else(|| get_cpu_count().max(1));

        let env = create_and_update_lmdb_env(env_factory, env_options)?;

        let ledger = Ledger::new(
            env,
            ledger_constants,
            rep_weights.clone(),
            stats.clone(),
            self.thread_count,
            batch_validation_threads,
            self.consistency_check,
        )?;

        if let Some(sender) = self.sender {
            *ledger.publish.write().unwrap() = Some(sender);
        }

        Ok(ledger)
    }
}
