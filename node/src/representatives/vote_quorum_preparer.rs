use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex, RwLock},
};

use rsnano_ledger::RepWeightCache;
use rsnano_nullable_clock::Timestamp;
use rsnano_types::{Amount, PublicKey};

use super::{OnlineReps, QuorumSpecs};

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

struct ObservedOnlineReps {
    rep_weights: Arc<RepWeightCache>,
    representative_weight_minimum: Amount,
    online_weight_minimum: Amount,
    trended_weight: Amount,
    online_weight: Amount,
    by_time: BTreeMap<Timestamp, Vec<PublicKey>>,
    by_account: HashMap<PublicKey, Timestamp>,
}

impl ObservedOnlineReps {
    fn new(online_reps: &OnlineReps) -> Self {
        let snapshot = online_reps.quorum_tracker_snapshot();
        let mut by_time = BTreeMap::new();
        let mut by_account = HashMap::new();

        for (rep, observed_at) in snapshot.observed_reps {
            by_time.entry(observed_at).or_insert_with(Vec::new).push(rep);
            by_account.insert(rep, observed_at);
        }

        Self {
            rep_weights: snapshot.rep_weights,
            representative_weight_minimum: snapshot.representative_weight_minimum,
            online_weight_minimum: snapshot.online_weight_minimum,
            trended_weight: snapshot.trended_weight,
            online_weight: snapshot.online_weight,
            by_time,
            by_account,
        }
    }

    fn published(&self) -> PublishedQuorumState {
        let trended_or_minimum_weight = self.trended_or_minimum_weight();
        PublishedQuorumState {
            minimum_principal_weight: trended_or_minimum_weight / 1000,
            quorum_specs: QuorumSpecs {
                online_weight: trended_or_minimum_weight,
                quorum_delta: OnlineReps::quorum_delta_for(
                    self.online_weight,
                    trended_or_minimum_weight,
                ),
            },
            trended_or_minimum_weight,
            online_weight: self.online_weight,
        }
    }

    fn trended_or_minimum_weight(&self) -> Amount {
        self.trended_weight.max(self.online_weight_minimum)
    }

    fn vote_observed(&mut self, voter: PublicKey, now: Timestamp) -> bool {
        if self.rep_weights.weight(&voter) < self.representative_weight_minimum {
            return false;
        }

        if let Some(previous) = self.by_account.insert(voter, now) {
            let entries = self.by_time.get_mut(&previous).unwrap();
            if entries.len() == 1 {
                self.by_time.remove(&previous);
            } else {
                entries.retain(|rep| rep != &voter);
            }
            self.by_time.entry(now).or_default().push(voter);
            false
        } else {
            self.by_time.entry(now).or_default().push(voter);
            self.online_weight += self.rep_weights.weight(&voter);
            true
        }
    }

    fn trim(&mut self, now: Timestamp) -> bool {
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
                self.online_weight -= self.rep_weights.weight(&rep);
            }
            changed = true;
        }

        changed
    }
}

/// Owns synchronous online-representative observation for vote-path quorum preparation.
pub struct VoteQuorumPreparer {
    observed_reps: Mutex<ObservedOnlineReps>,
    published: RwLock<PublishedQuorumState>,
}

impl VoteQuorumPreparer {
    pub fn new(online_reps: Arc<Mutex<OnlineReps>>) -> Self {
        let observed_reps = ObservedOnlineReps::new(&online_reps.lock().unwrap());
        let published = observed_reps.published();
        Self {
            observed_reps: Mutex::new(observed_reps),
            published: RwLock::new(published),
        }
    }

    pub fn prepare(
        &self,
        voter: PublicKey,
        is_active: bool,
        now: Timestamp,
    ) -> VoteQuorumPreparation {
        if is_active {
            let mut observed = self.observed_reps.lock().unwrap();
            observed.vote_observed(voter, now);
            let published = observed.published();
            *self.published.write().unwrap() = published.clone();
            return VoteQuorumPreparation {
                minimum_principal_weight: published.minimum_principal_weight,
                quorum_specs: published.quorum_specs,
            };
        }

        self.current_preparation()
    }

    pub fn trended_or_minimum_weight(&self) -> Amount {
        self.published.read().unwrap().trended_or_minimum_weight
    }

    pub fn minimum_principal_weight(&self) -> Amount {
        self.published.read().unwrap().minimum_principal_weight
    }

    pub fn quorum_delta(&self) -> Amount {
        self.published.read().unwrap().quorum_specs.quorum_delta
    }

    pub fn online_weight(&self) -> Amount {
        self.published.read().unwrap().online_weight
    }

    pub fn record_direct_observation(&self, voter: PublicKey, now: Timestamp) {
        let mut observed = self.observed_reps.lock().unwrap();
        observed.vote_observed(voter, now);
        *self.published.write().unwrap() = observed.published();
    }

    pub fn trim(&self, now: Timestamp) {
        let mut observed = self.observed_reps.lock().unwrap();
        if observed.trim(now) {
            *self.published.write().unwrap() = observed.published();
        }
    }

    pub fn set_trended(&self, trended: Amount) {
        let mut observed = self.observed_reps.lock().unwrap();
        observed.trended_weight = trended;
        *self.published.write().unwrap() = observed.published();
    }

    fn current_preparation(&self) -> VoteQuorumPreparation {
        let published = self.published.read().unwrap().clone();
        VoteQuorumPreparation {
            minimum_principal_weight: published.minimum_principal_weight,
            quorum_specs: published.quorum_specs,
        }
    }
}
