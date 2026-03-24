use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
};

use rsnano_nullable_clock::SteadyClock;

use rsnano_ledger::RepWeightCache;
use rsnano_types::{Amount, BlockHash, VoteError};
use rsnano_utils::sync::backpressure_channel::Sender;

use super::{ActiveElectionsContainer, AecEvent, FilteredVote, ReceivedVote, VoteApplicationEvent};
use crate::{consensus::ApplyVoteArgs, representatives::OnlineReps};

/// Applies a vote to an election
pub(crate) struct VoteApplier {
    active_elections: Arc<RwLock<ActiveElectionsContainer>>,
    event_senders: RwLock<Vec<Sender<AecEvent>>>,
    online_reps: Arc<Mutex<OnlineReps>>,
    clock: Arc<SteadyClock>,
    rep_weights: Arc<RepWeightCache>,
    is_dev_network: bool,
}

impl VoteApplier {
    pub(crate) fn new(
        active_elections: Arc<RwLock<ActiveElectionsContainer>>,
        online_reps: Arc<Mutex<OnlineReps>>,
        clock: Arc<SteadyClock>,
        rep_weights: Arc<RepWeightCache>,
        is_dev_network: bool,
    ) -> Self {
        Self {
            active_elections,
            event_senders: RwLock::new(Vec::new()),
            online_reps,
            clock,
            rep_weights,
            is_dev_network,
        }
    }

    pub fn add_event_sink(&self, sink: Sender<AecEvent>) {
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

        let minimum_pr_weight = self.online_reps.lock().unwrap().minimum_principal_weight();
        let voter_weight = self.rep_weights.weight(&vote.voter);

        if !self.is_dev_network && voter_weight <= minimum_pr_weight {
            // Ignore votes from reps below min PR weight!
            return vote
                .filtered_blocks()
                .map(|h| (*h, Err(VoteError::Indeterminate)))
                .collect();
        }

        let is_active = {
            let active = self.active_elections.read().unwrap();
            vote.filtered_blocks()
                .any(|hash| active.is_active_hash(hash))
        };

        let now = self.clock.now();

        let quorum_specs = {
            let mut online = self.online_reps.lock().unwrap();
            if is_active {
                // Representative is defined as online if replying to live votes or rep_crawler queries.
                // The rep weights have to be updated before the votes are processed!
                online.vote_observed(vote.voter, now);
            }
            online.quorum_specs()
        };

        let results = {
            let mut active = self.active_elections.write().unwrap();
            let rep_weights = self.rep_weights.read();
            active.apply_vote(ApplyVoteArgs {
                vote,
                rep_weights: &rep_weights,
                quorum_specs: &quorum_specs,
                now,
            })
        };

        self.notify_vote_application_events(&results.events);
        self.notify_vote_processed(vote, voter_weight, &results.per_block);
        results.per_block
    }

    fn notify_vote_application_events(&self, events: &[VoteApplicationEvent]) {
        for event in events {
            for sender in self.event_senders.read().unwrap().iter() {
                let event = match event {
                    VoteApplicationEvent::WinnerChanged(old_winner, new_winner) => {
                        AecEvent::WinnerChanged(*old_winner, new_winner.clone())
                    }
                    VoteApplicationEvent::ElectionConfirmed(election) => {
                        AecEvent::ElectionConfirmed(election.clone())
                    }
                };
                let _ = sender.send(event);
            }
        }
    }

    fn notify_vote_processed(
        &self,
        vote: &ReceivedVote,
        voter_weight: Amount,
        results: &HashMap<BlockHash, Result<(), VoteError>>,
    ) {
        for sender in self.event_senders.read().unwrap().iter() {
            let _ = sender.send(AecEvent::VoteProcessed(
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
    use crate::consensus::AecInsertRequest;
    use rsnano_types::{
        BlockPriority, PrivateKey, SavedBlock, UnixMillisTimestamp, Vote, VoteSource,
    };
    use rsnano_utils::sync::backpressure_channel::channel;
    use std::sync::mpsc::TryRecvError;

    #[test]
    fn update_online_weight_before_quorum_checks() {
        let block = SavedBlock::new_test_instance();
        let block_hash = block.hash();
        let rep_key = PrivateKey::from(1);
        let another_rep = PrivateKey::from(2);

        let rep_weights = Arc::new(RepWeightCache::default());
        rep_weights.put(rep_key.public_key(), Amount::nano(50_000_000));
        rep_weights.put(another_rep.public_key(), Amount::nano(65_000_000));

        let aec = Arc::new(RwLock::new(ActiveElectionsContainer::default()));
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

        aec.write()
            .unwrap()
            .insert(
                AecInsertRequest::new_priority(block, BlockPriority::new_test_instance()),
                clock.now(),
            )
            .unwrap();

        let vote_applier = VoteApplier::new(aec.clone(), online_reps, clock, rep_weights, false);

        let vote = ReceivedVote::new(
            Vote::new(&rep_key, UnixMillisTimestamp::new(123), 0, vec![block_hash]).into(),
            VoteSource::Live,
            None,
        );

        vote_applier.vote(&vote.into());

        let aec_guard = aec.read().unwrap();
        let election = aec_guard.election_for_block(&block_hash).unwrap();
        assert_eq!(election.winner_tally(), Amount::nano(50_000_000));

        // No quorum, because the vote of our rep has to be added to the online
        // weight before the quorum is checked!
        assert_eq!(election.has_quorum(), false);
    }

    #[test]
    fn emits_vote_application_events_from_service() {
        let block = SavedBlock::new_test_instance();
        let block_hash = block.hash();
        let rep_key = PrivateKey::from(1);

        let rep_weights = Arc::new(RepWeightCache::default());
        rep_weights.put(rep_key.public_key(), Amount::MAX);

        let aec = Arc::new(RwLock::new(ActiveElectionsContainer::default()));
        let online_reps = Arc::new(Mutex::new(
            OnlineReps::builder()
                .rep_weights(rep_weights.clone())
                .finish(),
        ));
        let clock = Arc::new(SteadyClock::new_null());

        aec.write()
            .unwrap()
            .insert(
                AecInsertRequest::new_priority(block, BlockPriority::new_test_instance()),
                clock.now(),
            )
            .unwrap();

        let vote_applier = VoteApplier::new(aec, online_reps, clock, rep_weights, false);
        let (tx, rx) = channel(8);
        vote_applier.add_event_sink(tx);

        let vote = ReceivedVote::new(
            Vote::new_final(&rep_key, vec![block_hash]).into(),
            VoteSource::Live,
            None,
        );

        let results = vote_applier.vote(&vote.into());

        assert_eq!(results.get(&block_hash), Some(&Ok(())));
        assert!(matches!(rx.try_recv(), Ok(AecEvent::ElectionConfirmed(_))));
        assert!(matches!(
            rx.try_recv(),
            Ok(AecEvent::VoteProcessed(_, _, _))
        ));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }
}
