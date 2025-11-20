use std::{
    collections::HashMap,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
};

use rsnano_types::{Amount, PublicKey};

use crate::{RepWeightCache, RepWeightStore, RepWeights};
use store_traits::LedgerWriteTxn;

/// Updates the representative weights in the ledger and in the in-memory cache
pub struct RepWeightsUpdater {
    weight_cache: Arc<RwLock<RepWeights>>,
    store: Arc<dyn RepWeightStore>,
    min_weight: Amount,
    stats: Arc<RepWeightWriterStats>,
}

impl RepWeightsUpdater {
    pub fn new(
        store: Arc<dyn RepWeightStore>,
        min_weight: Amount,
        cache: &RepWeightCache,
    ) -> Self {
        RepWeightsUpdater {
            weight_cache: cache.inner(),
            store,
            min_weight,
            stats: Arc::new(RepWeightWriterStats::default()),
        }
    }

    pub fn stats(&self) -> &Arc<RepWeightWriterStats> {
        &self.stats
    }

    /// Only use this method when loading rep weights from the database table
    pub fn copy_from(&self, other: &HashMap<PublicKey, Amount>) {
        let mut guard_this = self.weight_cache.write().unwrap();
        for (account, amount) in other {
            let prev_amount = self.get(&guard_this, account);
            self.put_cache(&mut guard_this, *account, prev_amount.wrapping_add(*amount));
        }
    }

    fn get(&self, weights: &HashMap<PublicKey, Amount>, account: &PublicKey) -> Amount {
        weights.get(account).cloned().unwrap_or_default()
    }

    pub fn representation_add(
        &self,
        tx: &mut dyn LedgerWriteTxn,
        representative: PublicKey,
        amount: Amount,
    ) {
        let previous_weight = self.store.get(tx, &representative).unwrap_or_default();
        let new_weight = previous_weight.wrapping_add(amount);
        self.put_store(tx, representative, previous_weight, new_weight);
        let mut guard = self.weight_cache.write().unwrap();
        self.put_cache(&mut guard, representative, new_weight);
    }

    fn put_cache(
        &self,
        weights: &mut HashMap<PublicKey, Amount>,
        representative: PublicKey,
        new_weight: Amount,
    ) {
        if new_weight < self.min_weight || new_weight.is_zero() {
            weights.remove(&representative);
        } else {
            weights.insert(representative, new_weight);
        }
    }

    fn put_store(
        &self,
        tx: &mut dyn LedgerWriteTxn,
        representative: PublicKey,
        previous_weight: Amount,
        new_weight: Amount,
    ) {
        if new_weight.is_zero() {
            if !previous_weight.is_zero() {
                self.store.del(&mut *tx, &representative);
            }
        } else {
            self.store.put(&mut *tx, representative, new_weight);
        }
    }

    /// Only use this method when loading rep weights from the database table!
    pub fn representation_put(&self, representative: PublicKey, weight: Amount) {
        let mut guard = self.weight_cache.write().unwrap();
        self.put_cache(&mut guard, representative, weight);
    }

    pub fn representation_add_dual(
        &self,
        tx: &mut dyn LedgerWriteTxn,
        rep_1: PublicKey,
        amount_1: Amount,
        rep_2: PublicKey,
        amount_2: Amount,
    ) {
        if rep_1 != rep_2 {
            let previous_weight_1 = self.store.get(tx, &rep_1).unwrap_or_default();
            let previous_weight_2 = self.store.get(tx, &rep_2).unwrap_or_default();
            let new_weight_1 = previous_weight_1.wrapping_add(amount_1);
            let new_weight_2 = previous_weight_2.wrapping_add(amount_2);
            self.put_store(tx, rep_1, previous_weight_1, new_weight_1);
            self.put_store(tx, rep_2, previous_weight_2, new_weight_2);
            let mut guard = self.weight_cache.write().unwrap();
            self.put_cache(&mut guard, rep_1, new_weight_1);
            self.put_cache(&mut guard, rep_2, new_weight_2);
        } else {
            self.representation_add(tx, rep_1, amount_1.wrapping_add(amount_2));
        }
    }
}

#[derive(Default)]
pub struct RepWeightWriterStats {
    optimistic_successes: AtomicU64,
    optimistic_conflicts: AtomicU64,
    pessimistic_fallbacks: AtomicU64,
    optimistic_active: AtomicU64,
    max_optimistic_concurrency: AtomicU64,
}

impl RepWeightWriterStats {
    pub fn optimistic_successes(&self) -> u64 {
        self.optimistic_successes.load(Relaxed)
    }

    pub fn optimistic_conflicts(&self) -> u64 {
        self.optimistic_conflicts.load(Relaxed)
    }

    pub fn pessimistic_fallbacks(&self) -> u64 {
        self.pessimistic_fallbacks.load(Relaxed)
    }

    pub fn max_optimistic_concurrency(&self) -> u64 {
        self.max_optimistic_concurrency.load(Relaxed)
    }

    pub fn start_optimistic_writer(&self) -> OptimisticWriterGuard<'_> {
        let active = self.optimistic_active.fetch_add(1, Relaxed) + 1;
        let mut observed = self.max_optimistic_concurrency.load(Relaxed);
        while active > observed {
            match self
                .max_optimistic_concurrency
                .compare_exchange(observed, active, Relaxed, Relaxed)
            {
                Ok(_) => break,
                Err(actual) => observed = actual,
            }
        }
        OptimisticWriterGuard { stats: self }
    }

    pub fn add_deltas(&self, successes: u64, conflicts: u64, fallbacks: u64) {
        self.optimistic_successes.fetch_add(successes, Relaxed);
        self.optimistic_conflicts.fetch_add(conflicts, Relaxed);
        self.pessimistic_fallbacks.fetch_add(fallbacks, Relaxed);
    }

    fn end_optimistic_writer(&self) {
        self.optimistic_active.fetch_sub(1, Relaxed);
    }
}

pub struct OptimisticWriterGuard<'a> {
    stats: &'a RepWeightWriterStats,
}

impl Drop for OptimisticWriterGuard<'_> {
    fn drop(&mut self) {
        self.stats.end_optimistic_writer();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_nullable_lmdb::LmdbEnvironment;
    use rsnano_store_lmdb::{LmdbLedgerWriteTxn, LmdbRepWeightStore};

    #[test]
    fn representation_changes() {
        let env = Arc::new(LmdbEnvironment::new_null());
        let lmdb_store = Arc::new(LmdbRepWeightStore::new(&env).unwrap());
        let store: Arc<dyn RepWeightStore> = lmdb_store.clone();
        let account = PublicKey::from(1);
        let rep_weights = RepWeightCache::new();
        let rep_weights_updater = RepWeightsUpdater::new(store, Amount::ZERO, &rep_weights);
        assert_eq!(rep_weights.weight(&account), Amount::ZERO);

        rep_weights_updater.representation_put(account, Amount::from(1));
        assert_eq!(rep_weights.weight(&account), Amount::from(1));

        rep_weights_updater.representation_put(account, Amount::from(2));
        assert_eq!(rep_weights.weight(&account), Amount::from(2));
    }

    #[test]
    fn delete_rep_weight_of_zero() {
        let representative = PublicKey::from(1);
        let weight = Amount::from(100);

        let env = Arc::new(LmdbEnvironment::new_null());
        let lmdb_store = Arc::new(LmdbRepWeightStore::new(&env).unwrap());
        let delete_tracker = lmdb_store.track_deletions();
        let store: Arc<dyn RepWeightStore> = lmdb_store.clone();
        let rep_weights = RepWeightCache::new();
        let rep_weights_updater = RepWeightsUpdater::new(store, Amount::ZERO, &rep_weights);
        rep_weights_updater.representation_put(representative, weight);
        let mut txn = LmdbLedgerWriteTxn::new(env.begin_write());
        lmdb_store.put(&mut txn, representative, weight);
        txn.into_inner().commit();
        let mut txn = LmdbLedgerWriteTxn::new(env.begin_write());

        // set weight to 0
        rep_weights_updater.representation_add(
            &mut txn,
            representative,
            Amount::ZERO.wrapping_sub(weight),
        );
        txn.into_inner().commit();

        assert_eq!(rep_weights.len(), 0);
        assert_eq!(delete_tracker.output(), vec![representative]);
    }

    #[test]
    fn delete_rep_weight_of_zero_dual() {
        let rep1 = PublicKey::from(1);
        let rep2 = PublicKey::from(2);
        let weight = Amount::from(100);

        let env = Arc::new(LmdbEnvironment::new_null());
        let lmdb_store = Arc::new(LmdbRepWeightStore::new(&env).unwrap());
        let delete_tracker = lmdb_store.track_deletions();
        let store: Arc<dyn RepWeightStore> = lmdb_store.clone();
        let rep_weights = RepWeightCache::new();
        let rep_weights_updater = RepWeightsUpdater::new(store, Amount::ZERO, &rep_weights);
        rep_weights_updater.representation_put(rep1, weight);
        rep_weights_updater.representation_put(rep2, weight);
        let mut txn = LmdbLedgerWriteTxn::new(env.begin_write());
        lmdb_store.put(&mut txn, rep1, weight);
        lmdb_store.put(&mut txn, rep2, weight);
        txn.into_inner().commit();
        let mut txn = LmdbLedgerWriteTxn::new(env.begin_write());

        // set weight to 0
        rep_weights_updater.representation_add_dual(
            &mut txn,
            rep1,
            Amount::ZERO.wrapping_sub(weight),
            rep2,
            Amount::ZERO.wrapping_sub(weight),
        );
        txn.into_inner().commit();

        assert_eq!(rep_weights.len(), 0);
        assert_eq!(delete_tracker.output(), vec![rep1, rep2]);
    }

    #[test]
    fn add_below_min_weight() {
        let env = Arc::new(LmdbEnvironment::new_null());
        let store = Arc::new(LmdbRepWeightStore::new(&env).unwrap());
        let put_tracker = store.track_puts();
        let mut txn = LmdbLedgerWriteTxn::new(env.begin_write());
        let representative = PublicKey::from(1);
        let min_weight = Amount::from(10);
        let rep_weight = Amount::from(9);
        let rep_weights = RepWeightCache::new();
        let rep_weights_updater = RepWeightsUpdater::new(store, min_weight, &rep_weights);

        rep_weights_updater.representation_add(&mut txn, representative, rep_weight);
        txn.into_inner().commit();

        assert_eq!(rep_weights.len(), 0);
        assert_eq!(put_tracker.output(), vec![(representative, rep_weight)]);
    }

    #[test]
    fn fall_below_min_weight() {
        let representative = PublicKey::from(1);
        let weight = Amount::from(11);
        let env = Arc::new(LmdbEnvironment::new_null());
        let store = Arc::new(LmdbRepWeightStore::new(&env).unwrap());
        {
            let mut seed_txn = LmdbLedgerWriteTxn::new(env.begin_write());
            store.put(&mut seed_txn, representative, weight);
            seed_txn.into_inner().commit();
        }
        let put_tracker = store.track_puts();
        let mut txn = LmdbLedgerWriteTxn::new(env.begin_write());
        let min_weight = Amount::from(10);
        let rep_weights = RepWeightCache::new();
        let rep_weights_updater = RepWeightsUpdater::new(store, min_weight, &rep_weights);

        rep_weights_updater.representation_add(
            &mut txn,
            representative,
            Amount::ZERO.wrapping_sub(Amount::from(2)),
        );
        txn.into_inner().commit();

        assert_eq!(rep_weights.len(), 0);
        assert_eq!(put_tracker.output(), vec![(representative, 9.into())]);
    }
}
