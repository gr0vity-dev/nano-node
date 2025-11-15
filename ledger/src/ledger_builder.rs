use std::{
    cmp::{max, min},
    path::PathBuf,
    sync::Arc,
};

use anyhow::anyhow;
use rsnano_types::Amount;
use rsnano_utils::get_cpu_count;
use rsnano_utils::stats::Stats;
use store_traits::{
    config::LedgerStoreConfig,
    ledger::{LedgerCache, LedgerStoreFactory},
};

use crate::{BootstrapWeights, Ledger, LedgerConstants, RepWeightCache};

pub struct LedgerBuilder<'a> {
    path: PathBuf,
    config: Option<LedgerStoreConfig>,
    store_factory: Option<&'a dyn LedgerStoreFactory>,
    bootstrap_weights: Option<BootstrapWeights>,
    stats: Option<Arc<Stats>>,
    min_rep_weight: Amount,
    ledger_constants: Option<LedgerConstants>,
    thread_count: usize,
}

impl<'a> LedgerBuilder<'a> {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            config: None,
            store_factory: None,
            bootstrap_weights: None,
            stats: None,
            min_rep_weight: Amount::ZERO,
            ledger_constants: None,
            thread_count: 0,
        }
    }

    pub fn store_factory(mut self, store_factory: &'a dyn LedgerStoreFactory) -> Self {
        self.store_factory = Some(store_factory);
        self
    }

    pub fn config(mut self, config: LedgerStoreConfig) -> Self {
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

    pub fn finish(mut self) -> anyhow::Result<Ledger> {
        let ledger_cache = Arc::new(LedgerCache::new());
        let bootstrap_weights = self.bootstrap_weights.unwrap_or_default();

        let rep_weights = Arc::new(RepWeightCache::with_bootstrap_weights(
            bootstrap_weights,
            ledger_cache.clone(),
        ));

        let config = self.config.unwrap_or_default();
        let store_factory = self
            .store_factory
            .ok_or_else(|| anyhow!("ledger store factory not configured"))?;

        let stats = self.stats.unwrap_or_else(|| Arc::new(Stats::default()));
        let ledger_constants = self
            .ledger_constants
            .unwrap_or_else(|| LedgerConstants::live());

        if self.thread_count == 0 {
            // Between 10 and 40 threads, scales well even in low power systems as long as actions are I/O bound
            self.thread_count = max(10, min(40, 11 * get_cpu_count()));
        }

        let store =
            store_factory.create_store(self.path, config, rep_weights.ledger_cache.clone())?;

        Ledger::new(
            store,
            ledger_constants,
            self.min_rep_weight,
            rep_weights.clone(),
            stats.clone(),
            self.thread_count,
        )
    }
}
