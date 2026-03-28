use std::{
    collections::{HashMap, HashSet},
    sync::RwLock,
    time::Duration,
};

use rsnano_nullable_clock::Timestamp;
use rsnano_types::{
    Account, Amount, Block, BlockHash, PublicKey, QualifiedRoot, SavedBlock, TimePriority,
    VoteError,
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
    root_container::{BucketCursor, ElectionHandle},
};
use crate::consensus::election::{
    AddForkResult, ConfirmationType, ConfirmedElection, Election, ElectionBehavior, ElectionState,
    VoteType,
};
use crate::consensus::election_schedulers::priority::bucket_count;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriorityActivationResult {
    Activated,
    ActivatedWithReplacement,
    Duplicate,
    RecentlyConfirmed,
    Stopped,
}

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

    pub fn priority_bucket_available(
        &self,
        bucket_id: usize,
        reserved_elections: usize,
        candidate_prio: TimePriority,
    ) -> bool {
        let aec = self.aec.read().unwrap();
        let bucket_len = aec.bucket_len(bucket_id);
        let lowest_prio = aec.lowest_priority(bucket_id);

        let can_reprioritize = lowest_prio
            .map(|(_, lowest)| candidate_prio > lowest)
            .unwrap_or(false);

        if can_reprioritize {
            return true;
        }

        if bucket_len >= reserved_elections {
            return false;
        }

        aec.vacancy() > 0
    }

    // --- Write forwarding ---

    pub fn set_observer(&self, observer: Sender<AecFact>) {
        self.aec.write().unwrap().set_observer(observer)
    }

    pub fn insert(&self, request: AecInsertRequest, now: Timestamp) -> Result<(), AecInsertError> {
        self.aec.write().unwrap().insert(request, now)
    }

    pub fn activate_priority(
        &self,
        bucket_id: usize,
        reserved_elections: usize,
        block: SavedBlock,
        priority: rsnano_types::BlockPriority,
        now: Timestamp,
    ) -> PriorityActivationResult {
        let root = block.qualified_root();
        let mut aec = self.aec.write().unwrap();

        if aec.find_bucket(&root) == Some(bucket_id) {
            return PriorityActivationResult::Duplicate;
        }

        let replaced = if aec.bucket_len(bucket_id) >= reserved_elections {
            aec.erase_lowest_prio_election(bucket_id);
            true
        } else {
            false
        };

        match aec.insert(AecInsertRequest::new_priority(block, priority), now) {
            Ok(()) if replaced => PriorityActivationResult::ActivatedWithReplacement,
            Ok(()) => PriorityActivationResult::Activated,
            Err(AecInsertError::RecentlyConfirmed) => PriorityActivationResult::RecentlyConfirmed,
            Err(AecInsertError::Duplicate) => PriorityActivationResult::Duplicate,
            Err(AecInsertError::Stopped) => PriorityActivationResult::Stopped,
        }
    }

    pub fn try_add_fork(&self, fork: &Block, fork_tally: Amount) -> bool {
        let root = fork.qualified_root();
        let Some(handle) = self.election_handle_for_root(&root) else {
            return false;
        };

        let result = handle.lock().try_add_fork(fork, fork_tally);
        match result {
            AddForkResult::Duplicate | AddForkResult::ElectionEnded => false,
            result => self
                .aec
                .write()
                .unwrap()
                .apply_fork_result(&root, &handle, fork, result),
        }
    }

    pub fn set_last_voted(&self, root: &QualifiedRoot, vote_type: VoteType, timestamp: Timestamp) {
        let handle = {
            let aec = self.aec.read().unwrap();
            aec.election_handle_for_root(root)
        };

        if let Some(handle) = handle {
            handle.lock().voted(vote_type, timestamp);
        }
    }

    pub fn apply_vote<'a>(
        &self,
        args: ApplyVoteArgs<'a>,
    ) -> HashMap<BlockHash, Result<(), VoteError>> {
        let mut filtered_blocks = args.vote.filtered_blocks().copied();
        let Some(block_hash) = filtered_blocks.next() else {
            return HashMap::new();
        };

        if filtered_blocks.next().is_none() {
            let (observer, result) = {
                let aec = self.aec.read().unwrap();
                (
                    aec.vote_observer(),
                    self.resolve_vote_result(&aec, block_hash),
                )
            };

            let helper = ApplyVoteHelper {
                args: &args,
                observer,
            };
            let mut results = HashMap::new();

            let (vote_result, counted_vote) = match result {
                ResolvedVoteResult::Apply(handle) => {
                    let apply_result = helper.apply_vote(&handle, &block_hash);
                    if let Some(cleanup) = apply_result.confirmed {
                        self.aec
                            .write()
                            .unwrap()
                            .cleanup_confirmed_election(cleanup);
                    }
                    (apply_result.vote_result, apply_result.vote_was_counted)
                }
                ResolvedVoteResult::Resolved(result) => (result, false),
            };

            if counted_vote {
                self.aec
                    .write()
                    .unwrap()
                    .count_applied_votes(args.vote.source, 1);
            }

            results.insert(block_hash, vote_result);
            return results;
        }

        let (observer, mut pending_votes, mut results) = {
            let aec = self.aec.read().unwrap();
            let mut pending_votes = Vec::new();
            let mut results = HashMap::new();
            let mut seen = HashSet::new();

            for block_hash in args.vote.filtered_blocks() {
                if !seen.insert(*block_hash) {
                    continue;
                }

                match self.resolve_vote_result(&aec, *block_hash) {
                    ResolvedVoteResult::Apply(handle) => pending_votes.push((*block_hash, handle)),
                    ResolvedVoteResult::Resolved(result) => {
                        results.insert(*block_hash, result);
                    }
                }
            }

            (aec.vote_observer(), pending_votes, results)
        };

        let helper = ApplyVoteHelper {
            args: &args,
            observer,
        };
        let mut counted_votes = 0;

        for (block_hash, handle) in pending_votes.drain(..) {
            let apply_result = helper.apply_vote(&handle, &block_hash);
            if apply_result.vote_was_counted {
                counted_votes += 1;
            }
            if let Some(cleanup) = apply_result.confirmed {
                self.aec
                    .write()
                    .unwrap()
                    .cleanup_confirmed_election(cleanup);
            }
            results.insert(block_hash, apply_result.vote_result);
        }

        if counted_votes > 0 {
            self.aec
                .write()
                .unwrap()
                .count_applied_votes(args.vote.source, counted_votes);
        }

        results
    }

    fn resolve_vote_result(
        &self,
        aec: &ActiveElectionsContainer,
        block_hash: BlockHash,
    ) -> ResolvedVoteResult {
        if let Some(handle) = aec.election_handle_for_block(&block_hash) {
            ResolvedVoteResult::Apply(handle)
        } else if aec.was_recently_confirmed(&block_hash) {
            ResolvedVoteResult::Resolved(Err(VoteError::Late))
        } else {
            ResolvedVoteResult::Resolved(Err(VoteError::Indeterminate))
        }
    }

    pub fn transition_time(&self, now: Timestamp) {
        let mut ended = Vec::new();

        self.for_each_round_robin_handle(|_, root, handle| {
            let mut election = handle.lock();
            election.transition_time(now);
            if election.state().has_ended() {
                ended.push(root);
            }
            true
        });

        let mut aec = self.aec.write().unwrap();
        aec.count_tick();
        for root in ended {
            aec.erase(&root);
        }
    }

    pub fn next_vote_in_bucket(
        &self,
        bucket_id: usize,
        vote_broadcast_interval: Duration,
        now: Timestamp,
    ) -> Option<(QualifiedRoot, VoteType, BlockHash)> {
        let mut cursor = None;
        while let Some((next_cursor, root, handle)) =
            self.next_bucket_handle(bucket_id, cursor.as_ref())
        {
            cursor = Some(next_cursor);
            let election = handle.lock();
            if election.can_vote(vote_broadcast_interval, now) {
                return Some((root, election.vote_type(), election.winner().hash()));
            }
        }

        None
    }

    pub fn stale_accounts(
        &self,
        now: Timestamp,
        stale_threshold: Duration,
        limit: usize,
    ) -> Vec<Account> {
        if limit == 0 {
            return Vec::new();
        }

        let mut stale_accounts = Vec::new();

        self.for_each_round_robin_handle(|_, _, handle| {
            let election = handle.lock();
            if election.start().elapsed(now) >= stale_threshold {
                stale_accounts.push(election.account());
            }
            stale_accounts.len() < limit
        });

        stale_accounts
    }

    pub fn election_snapshots(&self) -> Vec<Election> {
        self.aec.read().unwrap().iter_round_robin().collect()
    }

    pub fn active_election_snapshots(&self) -> Vec<Election> {
        self.election_snapshots()
            .into_iter()
            .filter(|e| e.state() == ElectionState::Active)
            .collect()
    }

    pub fn transition_active(&self, block_hash: &BlockHash) -> bool {
        let Some(handle) = self.election_handle_for_block(block_hash) else {
            return false;
        };
        handle.lock().transition_active();
        true
    }

    pub fn remove_votes<'a>(
        &self,
        root: &QualifiedRoot,
        voters: impl IntoIterator<Item = &'a PublicKey>,
    ) {
        let Some(handle) = self.election_handle_for_root(root) else {
            return;
        };

        let mut election = handle.lock();
        for voter in voters {
            election.remove_vote(voter);
        }
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
        let mut confirmed_results = Vec::with_capacity(confirmed.len());

        for (confirmed_block, source_election) in confirmed {
            let confirmed_election = if let Some(source) = source_election
                && confirmed_block.hash() == source.winner.hash()
            {
                source
            } else if let Some(handle) =
                self.election_handle_for_root(&confirmed_block.qualified_root())
            {
                let mut election = handle.lock();
                if election.winner().hash() == confirmed_block.hash() {
                    election.force_confirm();
                    election
                        .into_confirmed_election(now, ConfirmationType::ActiveConfirmationHeight)
                } else {
                    election.cancel();
                    ConfirmedElection::new(
                        confirmed_block.clone(),
                        ConfirmationType::ActiveConfirmationHeight,
                    )
                }
            } else {
                ConfirmedElection::new(
                    confirmed_block.clone(),
                    ConfirmationType::InactiveConfirmationHeight,
                )
            };

            confirmed_results.push((confirmed_block, confirmed_election));
        }

        let mut aec = self.aec.write().unwrap();
        for (block, election) in confirmed_results {
            aec.block_confirmed(block, election);
        }
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
        let Some(handle) = self.election_handle_for_root(root) else {
            return;
        };
        handle.lock().cancel();
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
        let (observer, handle) = {
            let aec = self.aec.read().unwrap();
            let handle = aec
                .election_handle_for_block(block_hash)
                .unwrap_or_else(|| {
                    panic!("Force confirm failed, because no active election was found")
                });
            (aec.vote_observer(), handle)
        };

        let confirmed_election = {
            let mut election = handle.lock();
            if !election.force_confirm() {
                return;
            }
            election.into_confirmed_election(now, ConfirmationType::ActiveConfirmedQuorum)
        };

        if let Some(observer) = observer {
            observer
                .send(AecFact::ElectionConfirmed(confirmed_election))
                .unwrap();
        }
    }

    pub fn simulate_event(&self, event: AecFact) {
        self.aec.read().unwrap().simulate_event(event)
    }

    fn election_handle_for_block(&self, block_hash: &BlockHash) -> Option<ElectionHandle> {
        self.aec
            .read()
            .unwrap()
            .election_handle_for_block(block_hash)
    }

    fn election_handle_for_root(&self, root: &QualifiedRoot) -> Option<ElectionHandle> {
        self.aec.read().unwrap().election_handle_for_root(root)
    }

    fn next_bucket_handle(
        &self,
        bucket_id: usize,
        after: Option<&BucketCursor>,
    ) -> Option<(BucketCursor, QualifiedRoot, ElectionHandle)> {
        let aec = self.aec.read().unwrap();
        aec.next_bucket(bucket_id, after)
            .map(|(cursor, (root, handle))| (cursor, root, handle))
    }

    fn for_each_round_robin_handle(
        &self,
        mut f: impl FnMut(usize, QualifiedRoot, ElectionHandle) -> bool,
    ) {
        let mut cursors = vec![None; bucket_count()];
        let mut next_bucket = bucket_count().saturating_sub(1);

        while let Some((bucket_id, cursor, root, handle)) =
            self.next_round_robin_handle(&cursors, next_bucket)
        {
            cursors[bucket_id] = Some(cursor);
            next_bucket = bucket_id.checked_sub(1).unwrap_or(bucket_count() - 1);
            if !f(bucket_id, root, handle) {
                break;
            }
        }
    }

    fn next_round_robin_handle(
        &self,
        cursors: &[Option<BucketCursor>],
        start_bucket: usize,
    ) -> Option<(usize, BucketCursor, QualifiedRoot, ElectionHandle)> {
        for offset in 0..bucket_count() {
            let bucket_id = (start_bucket + bucket_count() - offset) % bucket_count();
            if let Some((cursor, root, handle)) =
                self.next_bucket_handle(bucket_id, cursors[bucket_id].as_ref())
            {
                return Some((bucket_id, cursor, root, handle));
            }
        }
        None
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

enum ResolvedVoteResult {
    Apply(ElectionHandle),
    Resolved(Result<(), VoteError>),
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
    use rsnano_utils::sync::backpressure_channel::channel;
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
        let vote_a: ReceivedVote = ReceivedVote::new(
            Vote::new_final(&rep_key, vec![block_a.hash()]).into(),
            VoteSource::Live,
            None,
        );

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
        let vote_b: ReceivedVote = ReceivedVote::new(
            Vote::new_final(&rep_key, vec![block_b.hash()]).into(),
            VoteSource::Live,
            None,
        );
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

    #[test]
    fn apply_vote_removes_confirmed_election_immediately_after_vote() {
        let config = ActiveElectionsConfig {
            max_elections: 1,
            ..Default::default()
        };
        let aec = AecService::new(config, Duration::from_secs(1));
        let block = SavedBlock::new_test_instance();
        let now = Timestamp::new_test_instance();

        aec.insert(
            AecInsertRequest::new_priority(block.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        let rep_key = PrivateKey::from(1);
        let mut rep_weights = RepWeights::default();
        rep_weights.put(rep_key.public_key(), Amount::MAX);
        let vote: ReceivedVote = ReceivedVote::new(
            Vote::new_final(&rep_key, vec![block.hash()]).into(),
            VoteSource::Live,
            None,
        );

        let results = aec.apply_vote(ApplyVoteArgs {
            vote: &vote.into(),
            rep_weights: &rep_weights,
            quorum_specs: &QuorumSpecs::new_test_instance(),
            now,
        });

        assert_eq!(results.get(&block.hash()), Some(&Ok(())));
        assert!(!aec.is_active_root(&block.qualified_root()));
        assert!(aec.was_recently_confirmed(&block.hash()));
        assert_eq!(aec.vacancy(), 1);
    }

    #[test]
    fn apply_vote_preserves_confirmed_election_ended_observer_payload() {
        let aec = AecService::new_null();
        let block = SavedBlock::new_test_instance();
        let now = Timestamp::new_test_instance();
        let (tx, rx) = channel(8);

        aec.set_observer(tx);
        aec.insert(
            AecInsertRequest::new_priority(block.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        let rep_key = PrivateKey::from(1);
        let mut rep_weights = RepWeights::default();
        rep_weights.put(rep_key.public_key(), Amount::MAX);
        let vote: ReceivedVote = ReceivedVote::new(
            Vote::new_final(&rep_key, vec![block.hash()]).into(),
            VoteSource::Live,
            None,
        );

        let results = aec.apply_vote(ApplyVoteArgs {
            vote: &vote.into(),
            rep_weights: &rep_weights,
            quorum_specs: &QuorumSpecs::new_test_instance(),
            now,
        });

        assert_eq!(results.get(&block.hash()), Some(&Ok(())));

        let mut confirmed_seen = false;
        let mut ended_seen = false;
        for _ in 0..3 {
            match rx.recv().unwrap() {
                AecFact::ElectionStarted(_, _) => {}
                AecFact::ElectionConfirmed(confirmed) => {
                    confirmed_seen = true;
                    assert_eq!(confirmed.winner.hash(), block.hash());
                }
                AecFact::ElectionEnded(election) => {
                    ended_seen = true;
                    assert_eq!(election.qualified_root(), &block.qualified_root());
                    assert_eq!(election.winner().hash(), block.hash());
                    assert!(election.is_confirmed());
                }
                _ => panic!("unexpected event"),
            }
        }

        assert!(confirmed_seen);
        assert!(ended_seen);
    }

    #[test]
    fn apply_vote_returns_indeterminate_for_single_missing_hash() {
        let aec = AecService::new_null();
        let block = SavedBlock::new_test_instance();
        let now = Timestamp::new_test_instance();

        let vote: ReceivedVote = ReceivedVote::new(
            Vote::new_final(&PrivateKey::from(1), vec![block.hash()]).into(),
            VoteSource::Live,
            None,
        );

        let results = aec.apply_vote(ApplyVoteArgs {
            vote: &vote.into(),
            rep_weights: &RepWeights::default(),
            quorum_specs: &QuorumSpecs::new_test_instance(),
            now,
        });

        assert_eq!(results.len(), 1);
        assert_eq!(
            results.get(&block.hash()),
            Some(&Err(VoteError::Indeterminate))
        );
    }

    #[test]
    fn apply_vote_ignores_duplicate_single_hash_entries() {
        let aec = AecService::new_null();
        let block = SavedBlock::new_test_instance();
        let now = Timestamp::new_test_instance();

        aec.insert(
            AecInsertRequest::new_priority(block.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        let rep_key = PrivateKey::from(1);
        let mut rep_weights = RepWeights::default();
        rep_weights.put(rep_key.public_key(), Amount::MAX);
        let vote: ReceivedVote = ReceivedVote::new(
            Vote::new_final(&rep_key, vec![block.hash(), block.hash()]).into(),
            VoteSource::Live,
            None,
        );

        let results = aec.apply_vote(ApplyVoteArgs {
            vote: &vote.into(),
            rep_weights: &rep_weights,
            quorum_specs: &QuorumSpecs::new_test_instance(),
            now,
        });

        assert_eq!(results.len(), 1);
        assert_eq!(results.get(&block.hash()), Some(&Ok(())));
        assert!(!aec.is_active_root(&block.qualified_root()));
        assert!(aec.was_recently_confirmed(&block.hash()));
    }

    #[test]
    fn transition_active_does_not_hold_container_write_lock_while_waiting_for_other_election() {
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

        let started = Arc::new(AtomicBool::new(false));
        let started_clone = Arc::clone(&started);
        let aec_for_thread = Arc::clone(&aec);
        let block_hash = block_b.hash();

        let worker = thread::spawn(move || {
            started_clone.store(true, Ordering::Release);
            assert!(aec_for_thread.transition_active(&block_hash));
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

        drop(election_guard);
        worker.join().unwrap();
        assert_eq!(
            aec.election_for_block(&block_b.hash()).unwrap().state(),
            crate::consensus::election::ElectionState::Active
        );
    }

    #[test]
    fn confirm_dependent_elections_does_not_hold_container_write_lock_while_waiting_for_election() {
        let aec = Arc::new(AecService::new_null());
        let block = SavedBlock::new_test_instance_with_key(1);
        let now = Timestamp::new_test_instance();

        aec.insert(
            AecInsertRequest::new_priority(block.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        let (tx, rx) = channel(1);
        aec.set_observer(tx);

        let locked_handle = aec
            .aec
            .read()
            .unwrap()
            .election_handle_for_root(&block.qualified_root())
            .unwrap();
        let election_guard = locked_handle.lock();

        let started = Arc::new(AtomicBool::new(false));
        let started_clone = Arc::clone(&started);
        let aec_for_thread = Arc::clone(&aec);
        let confirmed = vec![(block.clone(), None)];

        let worker = thread::spawn(move || {
            started_clone.store(true, Ordering::Release);
            aec_for_thread.confirm_dependent_elections(confirmed, now);
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

        drop(election_guard);
        worker.join().unwrap();

        let event = rx.recv().unwrap();
        let AecFact::BlockConfirmed(confirmed_block, confirmed_election) = event else {
            panic!("expected BlockConfirmed event");
        };
        assert_eq!(confirmed_block.hash(), block.hash());
        assert_eq!(
            confirmed_election.confirmation_type,
            ConfirmationType::ActiveConfirmationHeight
        );
    }

    #[test]
    fn stale_accounts_streams_round_robin_order_with_limit() {
        let aec = AecService::new_null();
        let now = Timestamp::new_test_instance();
        let stale_start = now - Duration::from_secs(120);
        let blocks = [
            SavedBlock::new_test_instance_with_key(1),
            SavedBlock::new_test_instance_with_key(2),
            SavedBlock::new_test_instance_with_key(3),
        ];

        aec.insert(
            AecInsertRequest::new_priority(blocks[0].clone(), BlockPriority::new_test_instance()),
            stale_start,
        )
        .unwrap();
        aec.insert(
            AecInsertRequest::new_hinted(blocks[1].clone(), BlockPriority::new_test_instance()),
            stale_start,
        )
        .unwrap();
        aec.insert(
            AecInsertRequest::new_manual(blocks[2].clone(), BlockPriority::new_test_instance()),
            stale_start,
        )
        .unwrap();

        let expected: Vec<_> = aec
            .election_snapshots()
            .into_iter()
            .map(|election| election.account())
            .take(2)
            .collect();

        let stale_accounts = aec.stale_accounts(now, Duration::from_secs(60), 2);

        assert_eq!(stale_accounts, expected);
    }

    #[test]
    fn next_vote_in_bucket_streams_until_voteable_election() {
        let aec = AecService::new_null();
        let now = Timestamp::new_test_instance();
        let block_a = SavedBlock::new_test_instance_with_key(1);
        let block_b = SavedBlock::new_test_instance_with_key(2);
        let priority = BlockPriority::new_test_instance();

        aec.insert(
            AecInsertRequest::new_priority(block_a.clone(), priority),
            now - Duration::from_secs(10),
        )
        .unwrap();
        aec.insert(
            AecInsertRequest::new_priority(block_b.clone(), priority),
            now - Duration::from_secs(10),
        )
        .unwrap();

        let bucket_id = aec.find_bucket(&block_a.qualified_root()).unwrap();
        let vote_interval = Duration::from_secs(5);
        aec.set_last_voted(&block_a.qualified_root(), VoteType::NonFinal, now);

        let next_vote = aec
            .next_vote_in_bucket(bucket_id, vote_interval, now)
            .unwrap();

        assert_eq!(next_vote.0, block_b.qualified_root());
        assert_eq!(next_vote.1, VoteType::NonFinal);
        assert_eq!(next_vote.2, block_b.hash());
    }

    #[test]
    fn transition_time_streams_and_erases_ended_elections() {
        let aec = AecService::new_null();
        let now = Timestamp::new_test_instance();
        let transition_at = now + Duration::from_secs(31);
        let block_a = SavedBlock::new_test_instance_with_key(1);
        let block_b = SavedBlock::new_test_instance_with_key(2);

        aec.insert(
            AecInsertRequest::new_hinted(block_a.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();
        aec.insert(
            AecInsertRequest::new_optimistic(block_b.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        aec.transition_time(transition_at);

        assert!(!aec.is_active_root(&block_a.qualified_root()));
        assert!(!aec.is_active_root(&block_b.qualified_root()));
        assert!(aec.election_snapshots().is_empty());
    }
}
