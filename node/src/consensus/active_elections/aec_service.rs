use std::{
    collections::HashMap,
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
    time::Duration,
};

use rsnano_nullable_clock::Timestamp;
use rsnano_types::{
    Amount, Block, BlockHash, PublicKey, QualifiedRoot, SavedBlock, TimePriority, VoteError,
};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{StatsCollection, StatsSource},
    sync::backpressure_channel::Sender,
};

use super::{
    ActiveElectionsConfig, ActiveElectionsContainer, ActiveElectionsInfo, AecCooldownReason,
    AecFact, AecInsertError, AecInsertRequest, ApplyVoteArgs,
    apply_vote_helper::ApplyVoteHelper,
};
use crate::consensus::election::{ConfirmedElection, Election, ElectionBehavior, VoteType};

pub struct AecService {
    aec: RwLock<ActiveElectionsContainer>,
}

impl AecService {
    pub fn new(config: ActiveElectionsConfig, base_latency: Duration) -> Self {
        Self {
            aec: RwLock::new(ActiveElectionsContainer::new(config, base_latency)),
        }
    }

    pub fn new_null() -> Self {
        Self {
            aec: RwLock::new(ActiveElectionsContainer::default()),
        }
    }

    pub fn read(&self) -> RwLockReadGuard<'_, ActiveElectionsContainer> {
        self.aec.read().unwrap()
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, ActiveElectionsContainer> {
        self.aec.write().unwrap()
    }

    // --- Read forwarding ---

    pub fn election_for_root(&self, root: &QualifiedRoot) -> Option<Election> {
        self.aec.read().unwrap().election_for_root(root)
    }

    pub fn election_for_block(&self, block_hash: &BlockHash) -> Option<Election> {
        self.aec.read().unwrap().election_for_block(block_hash)
    }

    pub fn max_len(&self) -> usize {
        self.aec.read().unwrap().max_len()
    }

    pub fn len(&self) -> usize {
        self.aec.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.aec.read().unwrap().is_empty()
    }

    pub fn is_active_root(&self, root: &QualifiedRoot) -> bool {
        self.aec.read().unwrap().is_active_root(root)
    }

    pub fn is_active_hash(&self, block_hash: &BlockHash) -> bool {
        self.aec.read().unwrap().is_active_hash(block_hash)
    }

    pub fn was_recently_confirmed(&self, block_hash: &BlockHash) -> bool {
        self.aec.read().unwrap().was_recently_confirmed(block_hash)
    }

    pub fn count_by_behavior(&self, behavior: ElectionBehavior) -> usize {
        self.aec.read().unwrap().count_by_behavior(behavior)
    }

    pub fn bucket_len(&self, bucket_id: usize) -> usize {
        self.aec.read().unwrap().bucket_len(bucket_id)
    }

    pub fn find_bucket(&self, root: &QualifiedRoot) -> Option<usize> {
        self.aec.read().unwrap().find_bucket(root)
    }

    pub fn lowest_priority(&self, bucket_id: usize) -> Option<(QualifiedRoot, TimePriority)> {
        self.aec.read().unwrap().lowest_priority(bucket_id)
    }

    pub fn vacancy(&self) -> i64 {
        self.aec.read().unwrap().vacancy()
    }

    pub fn info(&self) -> ActiveElectionsInfo {
        self.aec.read().unwrap().info()
    }

    // --- Write forwarding ---

    pub fn set_observer(&self, observer: Sender<AecFact>) {
        self.aec.write().unwrap().set_observer(observer)
    }

    pub fn insert(&self, request: AecInsertRequest, now: Timestamp) -> Result<(), AecInsertError> {
        self.aec.write().unwrap().insert(request, now)
    }

    pub fn try_add_fork(&self, fork: &Block, fork_tally: Amount) -> bool {
        self.aec.write().unwrap().try_add_fork(fork, fork_tally)
    }

    pub fn set_last_voted(&self, root: &QualifiedRoot, vote_type: VoteType, timestamp: Timestamp) {
        self.aec
            .write()
            .unwrap()
            .set_last_voted(root, vote_type, timestamp)
    }

    pub fn apply_vote<'a>(
        &self,
        args: ApplyVoteArgs<'a>,
    ) -> HashMap<BlockHash, Result<(), VoteError>> {
        let (observer, mut pending_votes, mut results) = {
            let aec = self.aec.read().unwrap();
            let mut pending_votes = Vec::new();
            let mut results = HashMap::new();

            for block_hash in args.vote.filtered_blocks() {
                if results.contains_key(block_hash) {
                    continue;
                }

                if let Some(handle) = aec.election_handle_for_block(block_hash) {
                    pending_votes.push((*block_hash, handle));
                } else if aec.was_recently_confirmed(block_hash) {
                    results.insert(*block_hash, Err(VoteError::Late));
                } else {
                    results.insert(*block_hash, Err(VoteError::Indeterminate));
                }
            }

            (aec.vote_observer(), pending_votes, results)
        };

        let helper = ApplyVoteHelper {
            args: &args,
            observer,
        };
        let mut confirmed = Vec::new();
        let mut counted_votes = 0;

        for (block_hash, handle) in pending_votes.drain(..) {
            let apply_result = helper.apply_vote(&handle, &block_hash);
            if apply_result.vote_was_counted {
                counted_votes += 1;
            }
            if let Some(cleanup) = apply_result.confirmed {
                confirmed.push(cleanup);
            }
            results.insert(block_hash, apply_result.vote_result);
        }

        if counted_votes > 0 || !confirmed.is_empty() {
            let mut aec = self.aec.write().unwrap();
            aec.count_applied_votes(args.vote.source, counted_votes);
            aec.cleanup_confirmed_elections(confirmed);
        }

        results
    }

    pub fn transition_time(&self, now: Timestamp) {
        self.aec.write().unwrap().transition_time(now)
    }

    pub fn transition_active(&self, block_hash: &BlockHash) -> bool {
        self.aec.write().unwrap().transition_active(block_hash)
    }

    pub fn remove_votes<'a>(
        &self,
        root: &QualifiedRoot,
        voters: impl IntoIterator<Item = &'a PublicKey>,
    ) {
        self.aec.write().unwrap().remove_votes(root, voters)
    }

    pub fn erase_ended_elections(&self) {
        self.aec.write().unwrap().erase_ended_elections()
    }

    pub fn erase(&self, root: &QualifiedRoot) -> bool {
        self.aec.write().unwrap().erase(root)
    }

    pub fn erase_lowest_prio_election(&self, bucket_id: usize) {
        self.aec
            .write()
            .unwrap()
            .erase_lowest_prio_election(bucket_id)
    }

    pub fn confirm_dependent_elections(
        &self,
        confirmed: Vec<(SavedBlock, Option<ConfirmedElection>)>,
        now: Timestamp,
    ) {
        self.aec
            .write()
            .unwrap()
            .confirm_dependent_elections(confirmed, now)
    }

    pub fn remove_recently_confirmed(&self, block_hash: &BlockHash) {
        self.aec
            .write()
            .unwrap()
            .remove_recently_confirmed(block_hash)
    }

    pub fn set_cooldown(&self, cool_down: bool, reason: AecCooldownReason) {
        self.aec.write().unwrap().set_cooldown(cool_down, reason)
    }

    pub fn cancel(&self, root: &QualifiedRoot) {
        self.aec.write().unwrap().cancel(root)
    }

    pub fn cancel_all(&self) {
        self.aec.write().unwrap().cancel_all()
    }

    pub fn clear_recently_confirmed(&self) {
        self.aec.write().unwrap().clear_recently_confirmed()
    }

    pub fn stop(&self) {
        self.aec.write().unwrap().stop()
    }

    pub fn force_confirm(&self, block_hash: &BlockHash, now: Timestamp) {
        self.aec.write().unwrap().force_confirm(block_hash, now)
    }

    pub fn simulate_event(&self, event: AecFact) {
        self.aec.read().unwrap().simulate_event(event)
    }
}

impl StatsSource for AecService {
    fn collect_stats(&self, result: &mut StatsCollection) {
        self.aec.read().unwrap().collect_stats(result)
    }
}

impl ContainerInfoProvider for AecService {
    fn container_info(&self) -> ContainerInfo {
        self.aec.read().unwrap().container_info()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        consensus::{AecInsertRequest, ReceivedVote},
        representatives::QuorumSpecs,
    };
    use rsnano_ledger::RepWeights;
    use rsnano_types::{BlockPriority, PrivateKey, SavedBlock, Vote, VoteSource};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };

    #[test]
    fn apply_vote_does_not_hold_container_write_lock_while_waiting_for_other_election() {
        let aec = Arc::new(AecService::new_null());
        let block_a = SavedBlock::new_test_instance_with_key(1);
        let block_b = SavedBlock::new_test_instance_with_key(2);
        let now = Timestamp::new_test_instance();

        aec.insert(
            AecInsertRequest::new_priority(block_a.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();
        aec.insert(
            AecInsertRequest::new_priority(block_b.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        let locked_handle = aec
            .aec
            .read()
            .unwrap()
            .election_handle_for_block(&block_a.hash())
            .unwrap();
        let election_guard = locked_handle.lock();

        let rep_key = PrivateKey::from(1);
        let mut rep_weights = RepWeights::default();
        rep_weights.put(rep_key.public_key(), Amount::MAX);
        let quorum_specs = QuorumSpecs::new_test_instance();
        let started = Arc::new(AtomicBool::new(false));
        let started_clone = started.clone();
        let aec_for_thread = aec.clone();
        let vote_a: ReceivedVote =
            ReceivedVote::new(Vote::new_final(&rep_key, vec![block_a.hash()]).into(), VoteSource::Live, None);

        let worker = thread::spawn(move || {
            started_clone.store(true, Ordering::Release);
            aec_for_thread.apply_vote(ApplyVoteArgs {
                vote: &vote_a.into(),
                rep_weights: &rep_weights,
                quorum_specs: &quorum_specs,
                now,
            })
        });

        while !started.load(Ordering::Acquire) {
            thread::yield_now();
        }
        let mut container_write_available = false;
        for _ in 0..10_000 {
            if worker.is_finished() {
                break;
            }
            if aec.aec.try_write().is_ok() {
                container_write_available = true;
                break;
            }
            thread::yield_now();
        }

        assert!(container_write_available);

        let mut rep_weights_b = RepWeights::default();
        rep_weights_b.put(rep_key.public_key(), Amount::MAX);
        let vote_b: ReceivedVote =
            ReceivedVote::new(Vote::new_final(&rep_key, vec![block_b.hash()]).into(), VoteSource::Live, None);
        let results = aec.apply_vote(ApplyVoteArgs {
            vote: &vote_b.into(),
            rep_weights: &rep_weights_b,
            quorum_specs: &QuorumSpecs::new_test_instance(),
            now,
        });

        assert_eq!(results.get(&block_b.hash()), Some(&Ok(())));

        drop(election_guard);
        let first_results = worker.join().unwrap();
        assert_eq!(first_results.get(&block_a.hash()), Some(&Ok(())));
        assert!(aec.was_recently_confirmed(&block_a.hash()));
        assert!(aec.was_recently_confirmed(&block_b.hash()));
    }
}
