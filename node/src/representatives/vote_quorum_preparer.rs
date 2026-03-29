use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use rsnano_ledger::RepWeightCache;
use rsnano_nullable_clock::Timestamp;
use rsnano_types::{Amount, PublicKey};

use super::{OnlineReps, QuorumSpecs, QuorumTrackerStateSnapshot};

const OBSERVED_REP_SHARDS: usize = 16;

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

#[derive(Clone)]
struct ObservedRepShardState {
    by_time: BTreeMap<Timestamp, Vec<PublicKey>>,
    by_account: HashMap<PublicKey, Timestamp>,
    online_weight: Amount,
}

impl ObservedRepShardState {
    fn empty() -> Self {
        Self {
            by_time: BTreeMap::new(),
            by_account: HashMap::new(),
            online_weight: Amount::ZERO,
        }
    }

    fn vote_observed(&mut self, voter: PublicKey, now: Timestamp, weight: Amount) {
        if let Some(previous) = self.by_account.insert(voter, now) {
            let entries = self.by_time.get_mut(&previous).unwrap();
            if entries.len() == 1 {
                self.by_time.remove(&previous);
            } else {
                entries.retain(|rep| rep != &voter);
            }
            self.by_time.entry(now).or_default().push(voter);
        } else {
            self.by_time.entry(now).or_default().push(voter);
            self.online_weight += weight;
        }
    }

    fn trim(&mut self, now: Timestamp, rep_weights: &RepWeightCache) -> bool {
        let mut changed = false;
        let cutoff = now
            .checked_sub(std::time::Duration::from_secs(60 * 10))
            .unwrap_or_default();

        while let Some((&timestamp, _)) = self.by_time.first_key_value() {
            if timestamp >= cutoff {
                break;
            }

            let removed = self.by_time.pop_first().unwrap().1;
            for rep in removed {
                self.by_account.remove(&rep);
                self.online_weight -= rep_weights.weight(&rep);
            }
            changed = true;
        }

        changed
    }
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

struct ObservedRepShard {
    state: Mutex<ObservedRepShardState>,
    online_weight: AtomicAmount,
}

impl ObservedRepShard {
    fn new(state: ObservedRepShardState) -> Self {
        let online_weight = state.online_weight;
        Self {
            state: Mutex::new(state),
            online_weight: AtomicAmount::new(online_weight),
        }
    }
}

/// Owns synchronous online-representative observation for vote-path quorum preparation.
pub struct VoteQuorumPreparer {
    rep_weights: Arc<RepWeightCache>,
    representative_weight_minimum: Amount,
    online_weight_minimum: Amount,
    trended_weight: AtomicAmount,
    shards: Box<[ObservedRepShard]>,
    #[cfg(test)]
    mutation_hook: Option<Arc<dyn Fn(usize) + Send + Sync>>,
}

impl VoteQuorumPreparer {
    pub fn new(online_reps: Arc<Mutex<OnlineReps>>) -> Self {
        Self::from_snapshot(online_reps.lock().unwrap().quorum_tracker_snapshot())
    }

    #[cfg(test)]
    fn new_with_hook(
        online_reps: Arc<Mutex<OnlineReps>>,
        mutation_hook: Arc<dyn Fn(usize) + Send + Sync>,
    ) -> Self {
        let mut preparer = Self::from_snapshot(online_reps.lock().unwrap().quorum_tracker_snapshot());
        preparer.mutation_hook = Some(mutation_hook);
        preparer
    }

    fn from_snapshot(snapshot: QuorumTrackerStateSnapshot) -> Self {
        let mut shards: Vec<ObservedRepShardState> = (0..OBSERVED_REP_SHARDS)
            .map(|_| ObservedRepShardState::empty())
            .collect();

        for (rep, observed_at) in snapshot.observed_reps {
            let shard = &mut shards[Self::shard_index(rep)];
            shard.by_time.entry(observed_at).or_default().push(rep);
            shard.by_account.insert(rep, observed_at);
            shard.online_weight += snapshot.rep_weights.weight(&rep);
        }

        Self {
            rep_weights: snapshot.rep_weights,
            representative_weight_minimum: snapshot.representative_weight_minimum,
            online_weight_minimum: snapshot.online_weight_minimum,
            trended_weight: AtomicAmount::new(snapshot.trended_weight),
            shards: shards
                .into_iter()
                .map(ObservedRepShard::new)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
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
        for (shard_index, shard) in self.shards.iter().enumerate() {
            let mut guard = shard.state.lock().unwrap();
            if guard.trim(now, &self.rep_weights) {
                #[cfg(test)]
                if let Some(hook) = &self.mutation_hook {
                    hook(shard_index);
                }
                shard.online_weight.store(guard.online_weight);
            }
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

        let shard_index = Self::shard_index(voter);
        let shard = &self.shards[shard_index];
        let mut guard = shard.state.lock().unwrap();
        guard.vote_observed(voter, now, weight);
        #[cfg(test)]
        if let Some(hook) = &self.mutation_hook {
            hook(shard_index);
        }
        shard.online_weight.store(guard.online_weight);
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
        self.shards.iter().map(|shard| shard.online_weight.load()).sum()
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

    fn shard_index(voter: PublicKey) -> usize {
        voter.as_bytes()[31] as usize % OBSERVED_REP_SHARDS
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
        blocked_shard: usize,
        entered: Arc<(Mutex<bool>, Condvar)>,
        release: Arc<(Mutex<bool>, Condvar)>,
        rep_weights: Arc<RepWeightCache>,
    ) -> VoteQuorumPreparer {
        let online_reps = Arc::new(Mutex::new(
            OnlineReps::builder()
                .rep_weights(rep_weights)
                .finish(),
        ));
        VoteQuorumPreparer::new_with_hook(
            online_reps,
            Arc::new(move |shard_index| {
                if shard_index != blocked_shard {
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

    fn rep_on_different_shard(rep: &PrivateKey) -> PrivateKey {
        let current = VoteQuorumPreparer::shard_index(rep.public_key());
        for i in 2..100 {
            let candidate = PrivateKey::from(i);
            if VoteQuorumPreparer::shard_index(candidate.public_key()) != current {
                return candidate;
            }
        }
        panic!("could not find representative on a different shard");
    }

    #[test]
    fn prepare_includes_newly_observed_rep_in_same_vote_quorum() {
        let observed_rep = PrivateKey::from(1);
        let new_rep = PrivateKey::from(2);

        let rep_weights = Arc::new(RepWeightCache::default());
        rep_weights.put(observed_rep.public_key(), Amount::nano(65_000_000));
        rep_weights.put(new_rep.public_key(), Amount::nano(50_000_000));

        let online_reps = Arc::new(Mutex::new(
            OnlineReps::builder()
                .rep_weights(rep_weights)
                .finish(),
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
    fn active_prepare_does_not_wait_on_different_shard_mutation() {
        let first_rep = PrivateKey::from(1);
        let second_rep = rep_on_different_shard(&first_rep);

        let rep_weights = Arc::new(RepWeightCache::default());
        rep_weights.put(first_rep.public_key(), Amount::nano(80_000_000));
        rep_weights.put(second_rep.public_key(), Amount::nano(90_000_000));

        let entered = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let blocked_shard = VoteQuorumPreparer::shard_index(first_rep.public_key());
        let preparer = Arc::new(new_preparer_with_blocker(
            blocked_shard,
            entered.clone(),
            release.clone(),
            rep_weights,
        ));

        let blocked_prepare = {
            let preparer = preparer.clone();
            std::thread::spawn(move || {
                preparer.prepare(
                    first_rep.public_key(),
                    true,
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
                    second_rep.public_key(),
                    true,
                    Timestamp::new_test_instance() + Duration::from_secs(1),
                );
                tx.send(preparation.quorum_specs.quorum_delta).unwrap();
            })
        };

        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_ok(),
            "prepare on another shard should not wait on a blocked shard mutation"
        );

        release_blocker(&release);
        blocked_prepare.join().unwrap();
        concurrent_prepare.join().unwrap();
    }

    #[test]
    fn direct_observation_does_not_block_prepare_on_different_shard() {
        let direct_rep = PrivateKey::from(1);
        let active_vote_rep = rep_on_different_shard(&direct_rep);

        let rep_weights = Arc::new(RepWeightCache::default());
        rep_weights.put(direct_rep.public_key(), Amount::nano(80_000_000));
        rep_weights.put(active_vote_rep.public_key(), Amount::nano(90_000_000));

        let entered = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let blocked_shard = VoteQuorumPreparer::shard_index(direct_rep.public_key());
        let preparer = Arc::new(new_preparer_with_blocker(
            blocked_shard,
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
            "active prepare should not wait on direct observation for another shard"
        );

        release_blocker(&release);
        blocked_direct_observation.join().unwrap();
        concurrent_prepare.join().unwrap();
    }
}
