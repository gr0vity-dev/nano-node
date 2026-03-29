use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use rsnano_nullable_clock::SteadyClock;

use rsnano_ledger::RepWeightCache;
use rsnano_types::{Amount, BlockHash, VoteError};
use rsnano_utils::sync::backpressure_channel::Sender;

use super::{AecFact, AecService, FilteredVote, ReceivedVote};
use crate::{consensus::ApplyVoteArgs, representatives::VoteQuorumPreparer};

/// Applies a vote to an election
pub(crate) struct VoteApplier {
    active_elections: Arc<AecService>,
    event_senders: RwLock<Vec<Sender<AecFact>>>,
    quorum_preparer: Arc<VoteQuorumPreparer>,
    clock: Arc<SteadyClock>,
    rep_weights: Arc<RepWeightCache>,
    is_dev_network: bool,
}

impl VoteApplier {
    pub(crate) fn new(
        active_elections: Arc<AecService>,
        quorum_preparer: Arc<VoteQuorumPreparer>,
        clock: Arc<SteadyClock>,
        rep_weights: Arc<RepWeightCache>,
        is_dev_network: bool,
    ) -> Self {
        Self {
            active_elections,
            event_senders: RwLock::new(Vec::new()),
            quorum_preparer,
            clock,
            rep_weights,
            is_dev_network,
        }
    }

    pub fn add_event_sink(&self, sink: Sender<AecFact>) {
        self.event_senders.write().unwrap().push(sink);
    }

    pub fn stop(&self) {
        self.event_senders.write().unwrap().clear();
    }

    /// Route vote to associated elections
    /// Distinguishes replay votes, cannot be determined if the block is not in any election
    /// If 'filter' parameter is non-zero, only elections for the specified hash are notified.
    /// This eliminates duplicate processing when triggering votes from the vote_cache as the result of a specific election being created.
    pub fn vote(&self, vote: &FilteredVote) -> HashMap<BlockHash, Result<(), VoteError>> {
        debug_assert!(vote.validate().is_ok());
        let voter_weight = self.rep_weights.weight(&vote.voter);

        let is_active = vote
            .filtered_blocks()
            .any(|hash| self.active_elections.is_active_hash(hash));

        let now = self.clock.now();
        let preparation = self.quorum_preparer.prepare(vote.voter, is_active, now);

        if !self.is_dev_network && voter_weight <= preparation.minimum_principal_weight {
            // Ignore votes from reps below min PR weight!
            return vote
                .filtered_blocks()
                .map(|h| (*h, Err(VoteError::Indeterminate)))
                .collect();
        }

        let results = {
            let rep_weights = self.rep_weights.read();
            self.active_elections.apply_vote(ApplyVoteArgs {
                vote,
                rep_weights: &rep_weights,
                quorum_specs: &preparation.quorum_specs,
                now,
            })
        };

        self.notify_vote_processed(vote, voter_weight, &results);
        results
    }

    fn notify_vote_processed(
        &self,
        vote: &ReceivedVote,
        voter_weight: Amount,
        results: &HashMap<BlockHash, Result<(), VoteError>>,
    ) {
        for sender in self.event_senders.read().unwrap().iter() {
            let _ = sender.send(AecFact::VoteProcessed(
                vote.clone(),
                voter_weight,
                results.clone(),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Mutex, mpsc},
        time::Duration,
    };

    use crate::{
        consensus::{AecInsertRequest, AecService},
        representatives::{OnlineReps, VoteQuorumPreparer},
    };
    use rsnano_nullable_clock::Timestamp;
    use rsnano_types::{
        BlockPriority, PrivateKey, SavedBlock, UnixMillisTimestamp, Vote, VoteSource,
    };

    #[test]
    fn update_online_weight_before_quorum_checks() {
        let block = SavedBlock::new_test_instance();
        let block_hash = block.hash();
        let rep_key = PrivateKey::from(1);
        let another_rep = PrivateKey::from(2);

        let rep_weights = Arc::new(RepWeightCache::default());
        rep_weights.put(rep_key.public_key(), Amount::nano(50_000_000));
        rep_weights.put(another_rep.public_key(), Amount::nano(65_000_000));

        let aec = Arc::new(AecService::new_null());
        let online_reps = Arc::new(Mutex::new(
            OnlineReps::builder()
                .rep_weights(rep_weights.clone())
                .finish(),
        ));
        let clock = Arc::new(SteadyClock::new_null());

        online_reps
            .lock()
            .unwrap()
            .vote_observed(another_rep.public_key(), clock.now());

        assert_eq!(
            online_reps.lock().unwrap().quorum_delta(),
            Amount::nano(43_550_000)
        );

        aec.insert(
            AecInsertRequest::new_priority(block, BlockPriority::new_test_instance()),
            clock.now(),
        )
        .unwrap();

        let quorum_preparer = Arc::new(VoteQuorumPreparer::new(online_reps.clone()));
        let vote_applier =
            VoteApplier::new(aec.clone(), quorum_preparer, clock, rep_weights, false);

        let vote = ReceivedVote::new(
            Vote::new(&rep_key, UnixMillisTimestamp::new(123), 0, vec![block_hash]).into(),
            VoteSource::Live,
            None,
        );

        vote_applier.vote(&vote.into());

        let election = aec.election_for_block(&block_hash).unwrap();
        assert_eq!(election.winner_tally(), Amount::nano(50_000_000));

        // No quorum, because the vote of our rep has to be added to the online
        // weight before the quorum is checked!
        assert_eq!(election.has_quorum(), false);
    }

    #[test]
    fn quorum_preparation_does_not_wait_for_legacy_online_reps_mutex() {
        let rep = PrivateKey::from(1);
        let rep_weights = Arc::new(RepWeightCache::default());
        rep_weights.put(rep.public_key(), Amount::nano(80_000_000));

        let online_reps = Arc::new(Mutex::new(
            OnlineReps::builder().rep_weights(rep_weights).finish(),
        ));
        let quorum_preparer = Arc::new(VoteQuorumPreparer::new(online_reps.clone()));
        quorum_preparer.record_direct_observation(rep.public_key(), Timestamp::new_test_instance());

        let _legacy_guard = online_reps.lock().unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = {
            let quorum_preparer = quorum_preparer.clone();
            std::thread::spawn(move || {
                let preparation = quorum_preparer.prepare(
                    rep.public_key(),
                    true,
                    Timestamp::new_test_instance() + Duration::from_secs(1),
                );
                tx.send(preparation.quorum_specs.quorum_delta).unwrap();
            })
        };

        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_ok(),
            "quorum preparation should not wait on the legacy OnlineReps mutex"
        );
        worker.join().unwrap();
    }
}
