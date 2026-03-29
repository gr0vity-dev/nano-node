use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    },
};

use rsnano_ledger::RepWeightCache;
use rsnano_nullable_clock::Timestamp;
use rsnano_types::{Amount, PublicKey};

use super::{OnlineReps, QuorumSpecs, QuorumTrackerStateSnapshot};

pub struct VoteQuorumPreparation {
    pub minimum_principal_weight: Amount,
    pub quorum_specs: QuorumSpecs,
}

#[derive(Clone)]
struct PublishedQuorumState {
    minimum_principal_weight: Amount,
    quorum_specs: QuorumSpecs,
    trended_or_minimum_weight: Amount,
    online_weight: Amount,
}

struct AtomicAmount {
    writer: Mutex<()>,
    sequence: AtomicU64,
    low: AtomicU64,
    high: AtomicU64,
}

impl AtomicAmount {
    fn new(value: Amount) -> Self {
        let (low, high) = Self::split(value);
        Self {
            writer: Mutex::new(()),
            sequence: AtomicU64::new(0),
            low: AtomicU64::new(low),
            high: AtomicU64::new(high),
        }
    }

    fn load(&self) -> Amount {
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if before % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }

            let low = self.low.load(Ordering::Relaxed);
            let high = self.high.load(Ordering::Relaxed);

            let after = self.sequence.load(Ordering::Acquire);
            if before == after {
                return Self::join(low, high);
            }
        }
    }

    fn store(&self, value: Amount) {
        let _guard = self.writer.lock().unwrap();
        self.sequence.fetch_add(1, Ordering::AcqRel);
        let (low, high) = Self::split(value);
        self.low.store(low, Ordering::Relaxed);
        self.high.store(high, Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
    }

    fn split(value: Amount) -> (u64, u64) {
        let raw = value.number();
        (raw as u64, (raw >> 64) as u64)
    }

    fn join(low: u64, high: u64) -> Amount {
        Amount::raw(((high as u128) << 64) | low as u128)
    }
}

struct ObservedRepEntry {
    last_observed: AtomicI64,
    is_online: AtomicBool,
}

impl ObservedRepEntry {
    fn new(last_observed: Option<Timestamp>, is_online: bool) -> Self {
        Self {
            last_observed: AtomicI64::new(last_observed.map(|i| i.millis()).unwrap_or_default()),
            is_online: AtomicBool::new(is_online),
        }
    }
}

/// Owns synchronous online-representative observation for vote-path quorum preparation.
pub struct VoteQuorumPreparer {
    rep_weights: Arc<RepWeightCache>,
    representative_weight_minimum: Amount,
    online_weight_minimum: Amount,
    trended_weight: AtomicAmount,
    observed_reps: RwLock<Arc<HashMap<PublicKey, Arc<ObservedRepEntry>>>>,
    #[cfg(test)]
    mutation_hook: Option<Arc<dyn Fn(PublicKey) + Send + Sync>>,
}

impl VoteQuorumPreparer {
    pub fn new(online_reps: Arc<Mutex<OnlineReps>>) -> Self {
        Self::from_snapshot(online_reps.lock().unwrap().quorum_tracker_snapshot())
    }

    #[cfg(test)]
    pub(crate) fn new_with_hook(
        online_reps: Arc<Mutex<OnlineReps>>,
        mutation_hook: Arc<dyn Fn(PublicKey) + Send + Sync>,
    ) -> Self {
        let mut preparer =
            Self::from_snapshot(online_reps.lock().unwrap().quorum_tracker_snapshot());
        preparer.mutation_hook = Some(mutation_hook);
        preparer
    }

    fn from_snapshot(snapshot: QuorumTrackerStateSnapshot) -> Self {
        let rep_weights_cache = snapshot.rep_weights.clone();
        let observed_reps = snapshot
            .observed_reps
            .into_iter()
            .collect::<HashMap<_, _>>();

        let mut entries = HashMap::new();
        {
            let rep_weights = rep_weights_cache.read();
            for (rep, weight) in rep_weights.iter() {
                if *weight < snapshot.representative_weight_minimum
                    && !observed_reps.contains_key(rep)
                {
                    continue;
                }

                let last_observed = observed_reps.get(rep).copied();
                entries.insert(
                    *rep,
                    Arc::new(ObservedRepEntry::new(
                        last_observed,
                        last_observed.is_some(),
                    )),
                );
            }
        }

        Self {
            rep_weights: rep_weights_cache,
            representative_weight_minimum: snapshot.representative_weight_minimum,
            online_weight_minimum: snapshot.online_weight_minimum,
            trended_weight: AtomicAmount::new(snapshot.trended_weight),
            observed_reps: RwLock::new(Arc::new(entries)),
            #[cfg(test)]
            mutation_hook: None,
        }
    }

    pub fn prepare(
        &self,
        voter: PublicKey,
        is_active: bool,
        now: Timestamp,
    ) -> VoteQuorumPreparation {
        if is_active {
            self.observe_vote(voter, now);
        }

        self.current_preparation()
    }

    pub fn trended_or_minimum_weight(&self) -> Amount {
        self.current_published().trended_or_minimum_weight
    }

    pub fn minimum_principal_weight(&self) -> Amount {
        self.current_published().minimum_principal_weight
    }

    pub fn quorum_delta(&self) -> Amount {
        self.current_published().quorum_specs.quorum_delta
    }

    pub fn online_weight(&self) -> Amount {
        self.current_published().online_weight
    }

    pub fn record_direct_observation(&self, voter: PublicKey, now: Timestamp) {
        self.observe_vote(voter, now);
    }

    pub fn trim(&self, now: Timestamp) {
        let cutoff = now
            .checked_sub(std::time::Duration::from_secs(60 * 10))
            .unwrap_or_default()
            .millis();

        for entry in self.observed_reps.read().unwrap().values() {
            if !entry.is_online.load(Ordering::Acquire) {
                continue;
            }

            if entry.last_observed.load(Ordering::Acquire) >= cutoff {
                continue;
            }

            let _ =
                entry
                    .is_online
                    .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire);
        }
    }

    pub fn set_trended(&self, trended: Amount) {
        self.trended_weight.store(trended);
    }

    fn observe_vote(&self, voter: PublicKey, now: Timestamp) {
        let weight = self.rep_weights.weight(&voter);
        if weight < self.representative_weight_minimum {
            return;
        }

        let Some(entry) = self.observed_entry(voter) else {
            return;
        };

        #[cfg(test)]
        if let Some(hook) = &self.mutation_hook {
            hook(voter);
        }

        self.store_last_observed(&entry.last_observed, now);

        let _ = entry
            .is_online
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire);
    }

    fn current_preparation(&self) -> VoteQuorumPreparation {
        let published = self.current_published();
        VoteQuorumPreparation {
            minimum_principal_weight: published.minimum_principal_weight,
            quorum_specs: published.quorum_specs,
        }
    }

    fn current_published(&self) -> PublishedQuorumState {
        Self::published_state(
            self.total_online_weight(),
            self.trended_weight.load(),
            self.online_weight_minimum,
        )
    }

    fn total_online_weight(&self) -> Amount {
        self.observed_reps
            .read()
            .unwrap()
            .iter()
            .filter(|(_, entry)| entry.is_online.load(Ordering::Acquire))
            .map(|(rep, _)| self.rep_weights.weight(rep))
            .sum()
    }

    fn published_state(
        online_weight: Amount,
        trended_weight: Amount,
        online_weight_minimum: Amount,
    ) -> PublishedQuorumState {
        let trended_or_minimum_weight = trended_weight.max(online_weight_minimum);
        PublishedQuorumState {
            minimum_principal_weight: trended_or_minimum_weight / 1000,
            quorum_specs: QuorumSpecs {
                online_weight: trended_or_minimum_weight,
                quorum_delta: OnlineReps::quorum_delta_for(
                    online_weight,
                    trended_or_minimum_weight,
                ),
            },
            trended_or_minimum_weight,
            online_weight,
        }
    }

    fn observed_entry(&self, voter: PublicKey) -> Option<Arc<ObservedRepEntry>> {
        if let Some(entry) = self.observed_reps.read().unwrap().get(&voter) {
            return Some(entry.clone());
        }

        let weight = self.rep_weights.weight(&voter);
        if weight < self.representative_weight_minimum {
            return None;
        }

        let mut observed = self.observed_reps.write().unwrap();
        if let Some(entry) = observed.get(&voter) {
            return Some(entry.clone());
        }

        let mut updated = (**observed).clone();
        let entry = Arc::new(ObservedRepEntry::new(None, false));
        updated.insert(voter, entry.clone());
        *observed = Arc::new(updated);
        Some(entry)
    }

    fn store_last_observed(&self, target: &AtomicI64, now: Timestamp) {
        let now = now.millis();
        let mut current = target.load(Ordering::Acquire);
        while current < now {
            match target.compare_exchange(current, now, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(updated) => current = updated,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Condvar, mpsc},
        time::Duration,
    };

    use rsnano_types::PrivateKey;

    fn wait_until_blocked(entered: &Arc<(Mutex<bool>, Condvar)>, timeout: Duration) {
        let (lock, condition) = &**entered;
        let blocked = condition
            .wait_timeout_while(lock.lock().unwrap(), timeout, |blocked| !*blocked)
            .unwrap()
            .0;
        assert!(*blocked, "timed out waiting for blocked mutation");
    }

    fn release_blocker(release: &Arc<(Mutex<bool>, Condvar)>) {
        let (lock, condition) = &**release;
        *lock.lock().unwrap() = true;
        condition.notify_all();
    }

    fn new_preparer_with_blocker(
        blocked_rep: PublicKey,
        entered: Arc<(Mutex<bool>, Condvar)>,
        release: Arc<(Mutex<bool>, Condvar)>,
        rep_weights: Arc<RepWeightCache>,
    ) -> VoteQuorumPreparer {
        let online_reps = Arc::new(Mutex::new(
            OnlineReps::builder().rep_weights(rep_weights).finish(),
        ));
        VoteQuorumPreparer::new_with_hook(
            online_reps,
            Arc::new(move |voter| {
                if voter != blocked_rep {
                    return;
                }

                let (entered_lock, entered_condition) = &*entered;
                *entered_lock.lock().unwrap() = true;
                entered_condition.notify_all();

                let (release_lock, release_condition) = &*release;
                let _guard = release_condition
                    .wait_timeout_while(
                        release_lock.lock().unwrap(),
                        Duration::from_secs(1),
                        |released| !*released,
                    )
                    .unwrap()
                    .0;
            }),
        )
    }

    fn rep_on_same_shard(rep: &PrivateKey) -> PrivateKey {
        let current = rep.public_key().as_bytes()[31] as usize % 16;
        for i in 2..100 {
            let candidate = PrivateKey::from(i);
            if candidate.public_key().as_bytes()[31] as usize % 16 == current {
                return candidate;
            }
        }
        panic!("could not find representative on the same shard");
    }

    #[test]
    fn prepare_includes_newly_observed_rep_in_same_vote_quorum() {
        let observed_rep = PrivateKey::from(1);
        let new_rep = PrivateKey::from(2);

        let rep_weights = Arc::new(RepWeightCache::default());
        rep_weights.put(observed_rep.public_key(), Amount::nano(65_000_000));
        rep_weights.put(new_rep.public_key(), Amount::nano(50_000_000));

        let online_reps = Arc::new(Mutex::new(
            OnlineReps::builder().rep_weights(rep_weights).finish(),
        ));
        online_reps
            .lock()
            .unwrap()
            .vote_observed(observed_rep.public_key(), Timestamp::new_test_instance());

        let preparer = VoteQuorumPreparer::new(online_reps);
        let before = preparer.current_preparation().quorum_specs.quorum_delta;

        let after = preparer
            .prepare(
                new_rep.public_key(),
                true,
                Timestamp::new_test_instance() + Duration::from_secs(1),
            )
            .quorum_specs
            .quorum_delta;

        assert_eq!(preparer.online_weight(), Amount::nano(115_000_000));
        assert_eq!(preparer.quorum_delta(), after);
        assert_eq!(before, Amount::nano(43_550_000));
        assert_eq!(after, Amount::nano(77_050_000));
    }

    #[test]
    fn active_prepare_does_not_wait_on_other_vote_observation() {
        let first_rep = PrivateKey::from(1);
        let second_rep = rep_on_same_shard(&first_rep);

        let rep_weights = Arc::new(RepWeightCache::default());
        rep_weights.put(first_rep.public_key(), Amount::nano(80_000_000));
        rep_weights.put(second_rep.public_key(), Amount::nano(90_000_000));

        let entered = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let preparer = Arc::new(new_preparer_with_blocker(
            first_rep.public_key(),
            entered.clone(),
            release.clone(),
            rep_weights,
        ));

        let blocked_prepare = {
            let preparer = preparer.clone();
            std::thread::spawn(move || {
                preparer.prepare(first_rep.public_key(), true, Timestamp::new_test_instance());
            })
        };

        wait_until_blocked(&entered, Duration::from_millis(200));

        let (tx, rx) = mpsc::channel();
        let concurrent_prepare = {
            let preparer = preparer.clone();
            std::thread::spawn(move || {
                let preparation = preparer.prepare(
                    second_rep.public_key(),
                    true,
                    Timestamp::new_test_instance() + Duration::from_secs(1),
                );
                tx.send(preparation.quorum_specs.quorum_delta).unwrap();
            })
        };

        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_ok(),
            "prepare for another rep should not wait on blocked vote observation"
        );

        release_blocker(&release);
        blocked_prepare.join().unwrap();
        concurrent_prepare.join().unwrap();
    }

    #[test]
    fn direct_observation_does_not_block_prepare_on_other_rep() {
        let direct_rep = PrivateKey::from(1);
        let active_vote_rep = rep_on_same_shard(&direct_rep);

        let rep_weights = Arc::new(RepWeightCache::default());
        rep_weights.put(direct_rep.public_key(), Amount::nano(80_000_000));
        rep_weights.put(active_vote_rep.public_key(), Amount::nano(90_000_000));

        let entered = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let preparer = Arc::new(new_preparer_with_blocker(
            direct_rep.public_key(),
            entered.clone(),
            release.clone(),
            rep_weights,
        ));

        let blocked_direct_observation = {
            let preparer = preparer.clone();
            std::thread::spawn(move || {
                preparer.record_direct_observation(
                    direct_rep.public_key(),
                    Timestamp::new_test_instance(),
                );
            })
        };

        wait_until_blocked(&entered, Duration::from_millis(200));

        let (tx, rx) = mpsc::channel();
        let concurrent_prepare = {
            let preparer = preparer.clone();
            std::thread::spawn(move || {
                let preparation = preparer.prepare(
                    active_vote_rep.public_key(),
                    true,
                    Timestamp::new_test_instance() + Duration::from_secs(1),
                );
                tx.send(preparation.quorum_specs.quorum_delta).unwrap();
            })
        };

        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_ok(),
            "active prepare should not wait on direct observation for another rep"
        );

        release_blocker(&release);
        blocked_direct_observation.join().unwrap();
        concurrent_prepare.join().unwrap();
    }

    #[test]
    fn online_weight_uses_current_rep_weight_cache_for_tracked_rep() {
        let rep = PrivateKey::from(1);
        let rep_weights = Arc::new(RepWeightCache::default());
        rep_weights.put(rep.public_key(), Amount::nano(80_000_000));

        let online_reps = Arc::new(Mutex::new(
            OnlineReps::builder()
                .rep_weights(rep_weights.clone())
                .finish(),
        ));
        online_reps
            .lock()
            .unwrap()
            .vote_observed(rep.public_key(), Timestamp::new_test_instance());

        let preparer = VoteQuorumPreparer::new(online_reps);
        assert_eq!(preparer.online_weight(), Amount::nano(80_000_000));

        rep_weights.put(rep.public_key(), Amount::nano(95_000_000));

        assert_eq!(preparer.online_weight(), Amount::nano(95_000_000));
        assert_eq!(preparer.quorum_delta(), Amount::nano(63_650_000));
    }

    #[test]
    fn prepare_uses_current_rep_weight_cache_for_already_tracked_rep() {
        let tracked_rep = PrivateKey::from(1);
        let voter = PrivateKey::from(2);
        let rep_weights = Arc::new(RepWeightCache::default());
        rep_weights.put(tracked_rep.public_key(), Amount::nano(80_000_000));
        rep_weights.put(voter.public_key(), Amount::nano(50_000_000));

        let online_reps = Arc::new(Mutex::new(
            OnlineReps::builder()
                .rep_weights(rep_weights.clone())
                .finish(),
        ));
        online_reps
            .lock()
            .unwrap()
            .vote_observed(tracked_rep.public_key(), Timestamp::new_test_instance());

        let preparer = VoteQuorumPreparer::new(online_reps);
        let before = preparer.quorum_delta();

        rep_weights.put(tracked_rep.public_key(), Amount::nano(95_000_000));

        let after = preparer
            .prepare(
                voter.public_key(),
                true,
                Timestamp::new_test_instance() + Duration::from_secs(1),
            )
            .quorum_specs
            .quorum_delta;

        assert_eq!(before, Amount::nano(53_600_000));
        assert_eq!(after, Amount::nano(97_150_000));
        assert_eq!(preparer.online_weight(), Amount::nano(145_000_000));
        assert_eq!(preparer.quorum_delta(), after);
    }
}
