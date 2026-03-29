use std::sync::{Arc, Mutex};

use rsnano_nullable_clock::Timestamp;
use rsnano_types::{Amount, PublicKey};

use super::{OnlineReps, QuorumSpecs};

pub struct VoteQuorumPreparation {
    pub minimum_principal_weight: Amount,
    pub quorum_specs: QuorumSpecs,
}

/// Owns synchronous online-representative observation for vote-path quorum preparation.
pub struct VoteQuorumPreparer {
    online_reps: Arc<Mutex<OnlineReps>>,
}

impl VoteQuorumPreparer {
    pub fn new(online_reps: Arc<Mutex<OnlineReps>>) -> Self {
        Self { online_reps }
    }

    pub fn prepare(
        &self,
        voter: PublicKey,
        is_active: bool,
        now: Timestamp,
    ) -> VoteQuorumPreparation {
        let mut online = self.online_reps.lock().unwrap();
        let minimum_principal_weight = online.minimum_principal_weight();

        if is_active {
            // A newly observed rep must affect quorum before the tally is checked.
            online.vote_observed(voter, now);
        }

        VoteQuorumPreparation {
            minimum_principal_weight,
            quorum_specs: online.quorum_specs(),
        }
    }
}
