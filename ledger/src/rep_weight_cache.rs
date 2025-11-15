use std::{
    collections::HashMap,
    mem::size_of,
    ops::{Deref, DerefMut},
    sync::{
        Arc, RwLock, RwLockReadGuard,
        atomic::{AtomicBool, Ordering},
    },
};

use rsnano_types::{Account, Amount, PublicKey};
use rsnano_utils::container_info::ContainerInfo;
use store_traits::ledger::LedgerCache;

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct RepWeights(HashMap<PublicKey, Amount>);

impl RepWeights {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn weight(&self, rep: &PublicKey) -> Amount {
        self.get(rep).cloned().unwrap_or_default()
    }
}

impl Deref for RepWeights {
    type Target = HashMap<PublicKey, Amount>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for RepWeights {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Default)]
pub struct BootstrapWeights {
    pub weights: RepWeights,
    pub max_blocks: u64,
}

/// Returns the cached vote weight for the given representative.
/// If the weight is below the cache limit it returns 0.
/// During bootstrap it returns the preconfigured bootstrap weights.
pub struct RepWeightCache {
    weights: Arc<RwLock<RepWeights>>,
    bootstrap_weights: RwLock<RepWeights>,
    max_blocks: u64,
    pub ledger_cache: Arc<LedgerCache>,
    check_bootstrap_weights: AtomicBool,
}

impl RepWeightCache {
    pub fn new() -> Self {
        Self {
            weights: Arc::new(RwLock::new(RepWeights::new())),
            bootstrap_weights: RwLock::new(RepWeights::new()),
            max_blocks: 0,
            ledger_cache: Arc::new(LedgerCache::new()),
            check_bootstrap_weights: AtomicBool::new(false),
        }
    }

    pub fn with_bootstrap_weights(
        bootstrap_weights: BootstrapWeights,
        ledger_cache: Arc<LedgerCache>,
    ) -> Self {
        Self {
            weights: Arc::new(RwLock::new(RepWeights::new())),
            bootstrap_weights: RwLock::new(bootstrap_weights.weights),
            max_blocks: bootstrap_weights.max_blocks,
            ledger_cache,
            check_bootstrap_weights: AtomicBool::new(true),
        }
    }

    pub fn read(&self) -> RwLockReadGuard<'_, RepWeights> {
        if self.use_bootstrap_weights() {
            self.bootstrap_weights.read().unwrap()
        } else {
            self.weights.read().unwrap()
        }
    }

    pub fn use_bootstrap_weights(&self) -> bool {
        if self.check_bootstrap_weights.load(Ordering::SeqCst) {
            if self.ledger_cache.block_count.load(Ordering::SeqCst) < self.max_blocks {
                return true;
            } else {
                self.check_bootstrap_weights.store(false, Ordering::SeqCst);
            }
        }
        false
    }

    pub fn weight(&self, rep: &PublicKey) -> Amount {
        let weights = if self.use_bootstrap_weights() {
            &self.bootstrap_weights
        } else {
            &self.weights
        };

        weights
            .read()
            .unwrap()
            .get(rep)
            .cloned()
            .unwrap_or_default()
    }

    pub fn bootstrap_weight_max_blocks(&self) -> u64 {
        self.max_blocks
    }

    pub fn bootstrap_weights(&self) -> HashMap<PublicKey, Amount> {
        self.bootstrap_weights
            .read()
            .unwrap()
            .deref()
            .deref()
            .clone()
    }

    pub fn block_count(&self) -> u64 {
        self.ledger_cache.block_count.load(Ordering::SeqCst)
    }

    pub fn len(&self) -> usize {
        self.weights.read().unwrap().len()
    }

    pub fn set(&self, account: PublicKey, weight: Amount) {
        self.weights.write().unwrap().insert(account, weight);
    }

    pub(super) fn inner(&self) -> Arc<RwLock<RepWeights>> {
        self.weights.clone()
    }

    pub fn container_info(&self) -> ContainerInfo {
        [("rep_weights", self.len(), size_of::<(Account, Amount)>())].into()
    }
}

impl From<RepWeights> for RepWeightCache {
    fn from(value: RepWeights) -> Self {
        let mut rep_weights_cache = RepWeightCache::new();
        rep_weights_cache.weights = RwLock::new(value).into();
        rep_weights_cache
    }
}
