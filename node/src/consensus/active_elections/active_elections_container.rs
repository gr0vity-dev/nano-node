use std::time::Duration;

use strum::EnumCount;

use rsnano_ledger::RepWeights;
use rsnano_nullable_clock::Timestamp;
use rsnano_types::{
    Amount, Block, BlockHash, BlockPriority, PublicKey, QualifiedRoot, Root, SavedBlock, VoteSource,
};
use rsnano_utils::container_info::{ContainerInfo, ContainerInfoProvider};

use crate::{
    consensus::{
        election::{
            AddForkResult, ConfirmationType, ConfirmedElection, Election, ElectionBehavior,
            VoteType,
        },
        election_schedulers::priority::PriorityBucketState,
        filtered_vote::FilteredVote,
    },
    representatives::QuorumSpecs,
};

use super::{
    ActiveElectionsInfo, AecActivateRequest, AecFact, AecFacts, AecInsertError, AecInsertRequest,
    Entry, RootContainer,
    apply_vote_helper::{ApplyVoteHelper, ApplyVoteResult},
};

pub struct ActiveElectionsContainer {
    roots: RootContainer,
    base_latency: Duration,
}

#[derive(Default)]
pub(crate) struct AecContainerDelta {
    pub behavior_counts: [i64; ElectionBehavior::COUNT],
    pub started_behaviors: [u64; ElectionBehavior::COUNT],
    pub stopped_elections: Vec<Election>,
    pub recently_confirmed: Vec<(QualifiedRoot, BlockHash)>,
    pub vote_counts: [u64; VoteSource::COUNT],
    pub ticked: u64,
    pub conflicts: u64,
    pub block_confirmations: [usize; ConfirmationType::COUNT],
    pub route_additions: Vec<(BlockHash, QualifiedRoot)>,
    pub route_removals: Vec<BlockHash>,
}

impl AecContainerDelta {
    fn election_started(&mut self, behavior: ElectionBehavior) {
        self.behavior_counts[behavior as usize] += 1;
        self.started_behaviors[behavior as usize] += 1;
    }

    fn election_stopped(&mut self, election: &Election) {
        self.behavior_counts[election.behavior() as usize] -= 1;
        self.stopped_elections.push(election.clone());
    }

    pub(crate) fn merge(&mut self, other: Self) {
        for (i, count) in other.behavior_counts.into_iter().enumerate() {
            self.behavior_counts[i] += count;
        }
        for (i, count) in other.started_behaviors.into_iter().enumerate() {
            self.started_behaviors[i] += count;
        }
        self.stopped_elections.extend(other.stopped_elections);
        self.recently_confirmed.extend(other.recently_confirmed);
        for (i, count) in other.vote_counts.into_iter().enumerate() {
            self.vote_counts[i] += count;
        }
        self.ticked += other.ticked;
        self.conflicts += other.conflicts;
        for (i, count) in other.block_confirmations.into_iter().enumerate() {
            self.block_confirmations[i] += count;
        }
        self.route_additions.extend(other.route_additions);
        self.route_removals.extend(other.route_removals);
    }

    fn connect(&mut self, hash: BlockHash, root: QualifiedRoot) {
        self.route_additions.push((hash, root));
    }

    fn disconnect_hash(&mut self, hash: BlockHash) {
        self.route_removals.push(hash);
    }

    fn disconnect_election(&mut self, election: &Election) {
        self.route_removals
            .extend(election.candidate_blocks().keys().copied());
    }
}

#[derive(Default)]
pub(crate) struct AecMutationResult {
    pub facts: AecFacts,
    pub delta: AecContainerDelta,
}

impl AecMutationResult {
    fn from_fact(fact: AecFact) -> Self {
        Self {
            facts: fact.into(),
            delta: AecContainerDelta::default(),
        }
    }

    pub(crate) fn merge(&mut self, other: Self) {
        self.facts.extend(other.facts);
        self.delta.merge(other.delta);
    }
}

impl ActiveElectionsContainer {
    pub fn new(base_latency: Duration) -> Self {
        Self {
            roots: RootContainer::default(),
            base_latency,
        }
    }

    pub(crate) fn priority_bucket_state(
        &self,
        bucket_id: usize,
        candidate_root: &QualifiedRoot,
        is_cooling_down: bool,
        vacancy: i64,
    ) -> PriorityBucketState {
        PriorityBucketState {
            active_len: self.roots.bucket_len(bucket_id),
            contains_candidate: self.is_active_root(candidate_root),
            lowest: self.roots.lowest_priority(bucket_id),
            is_cooling_down,
            vacancy,
        }
    }

    pub fn iter_round_robin(&self) -> impl Iterator<Item = &Election> {
        self.roots.iter().map(|i| &i.election)
    }

    pub fn iter_bucket(&self, bucket_id: usize) -> impl Iterator<Item = &Election> {
        self.roots.iter_bucket(bucket_id).map(|i| &i.election)
    }

    pub(crate) fn insert(
        &mut self,
        request: AecInsertRequest,
        now: Timestamp,
    ) -> Result<AecMutationResult, AecInsertError> {
        if let Some(delta) = self.try_upgrade_priority_election(&request)? {
            return Ok(AecMutationResult {
                facts: AecFacts::new(),
                delta,
            });
        }

        Ok(self.insert_new_election(request, now))
    }

    pub(crate) fn activate(
        &mut self,
        request: AecActivateRequest,
        now: Timestamp,
        is_cooling_down: bool,
        vacancy: i64,
    ) -> Result<AecMutationResult, AecInsertError> {
        let root = request.qualified_root();
        let transition_active = request.transitions_to_active();

        let facts = match request {
            AecActivateRequest::Priority {
                block,
                priority,
                bucket_index,
                reserved_elections,
            } => self.activate_priority(
                block,
                priority,
                bucket_index,
                reserved_elections,
                now,
                is_cooling_down,
                vacancy,
            )?,
            request => self.insert(request.into_insert_request(), now)?,
        };

        if transition_active {
            self.transition_active(&root);
        }

        Ok(facts)
    }

    pub fn set_last_voted(
        &mut self,
        root: &QualifiedRoot,
        vote_type: VoteType,
        timestamp: Timestamp,
    ) {
        let Some(entry) = self.roots.get_mut(root) else {
            return;
        };
        entry.election.voted(vote_type, timestamp);
    }

    pub(crate) fn next_vote_to_broadcast(
        &mut self,
        bucket_id: usize,
        vote_broadcast_interval: Duration,
        now: Timestamp,
    ) -> Option<(Root, BlockHash, VoteType)> {
        let vote_target = self.iter_bucket(bucket_id).find_map(|election| {
            if election.can_vote(vote_broadcast_interval, now) {
                Some((
                    election.qualified_root().clone(),
                    election.vote_type(),
                    election.winner().hash(),
                ))
            } else {
                None
            }
        });

        vote_target.map(|(qualified_root, vote_type, winner_hash)| {
            self.set_last_voted(&qualified_root, vote_type, now);
            (qualified_root.root, winner_hash, vote_type)
        })
    }

    fn try_upgrade_priority_election(
        &mut self,
        request: &AecInsertRequest,
    ) -> Result<Option<AecContainerDelta>, AecInsertError> {
        let (upgraded, previous_behavior) = self.roots.try_upgrade_to_priority_election(request);

        if upgraded {
            let mut delta = AecContainerDelta::default();
            delta.behavior_counts[previous_behavior.unwrap() as usize] -= 1;
            delta.behavior_counts[request.behavior as usize] += 1;
            Ok(Some(delta))
        } else if previous_behavior.is_some() {
            Err(AecInsertError::Duplicate)
        } else {
            Ok(None)
        }
    }

    fn insert_new_election(
        &mut self,
        request: AecInsertRequest,
        now: Timestamp,
    ) -> AecMutationResult {
        let root = request.block.qualified_root();
        let hash = request.block.hash();
        let election = Election::new(request.block, request.behavior, self.base_latency, now);

        self.roots.insert(Entry {
            root: root.clone(),
            election,
            priority: request.priority,
        });
        let mut delta = AecContainerDelta::default();
        delta.election_started(request.behavior);
        delta.connect(hash, root.clone());
        AecMutationResult {
            facts: AecFact::ElectionStarted(hash, root).into(),
            delta,
        }
    }

    fn activate_priority(
        &mut self,
        block: SavedBlock,
        priority: BlockPriority,
        bucket_index: usize,
        reserved_elections: usize,
        now: Timestamp,
        is_cooling_down: bool,
        vacancy: i64,
    ) -> Result<AecMutationResult, AecInsertError> {
        let candidate_root = block.qualified_root();
        let state =
            self.priority_bucket_state(bucket_index, &candidate_root, is_cooling_down, vacancy);
        let request = AecInsertRequest {
            block,
            behavior: ElectionBehavior::Priority,
            priority,
        };
        if state.contains_candidate {
            return self.insert(request, now);
        }

        if state.active_len >= reserved_elections {
            let Some((lowest_root, _)) = state.lowest else {
                debug_assert!(false, "priority replacement requires a lowest election");
                return Err(AecInsertError::Duplicate);
            };
            self.replace_lowest_priority(&lowest_root, request, now)
        } else {
            self.insert(request, now)
        }
    }

    pub(crate) fn try_add_fork(
        &mut self,
        fork: &Block,
        fork_tally: Amount,
    ) -> (bool, AecMutationResult) {
        let Some(entry) = self.roots.get_mut(&fork.qualified_root()) else {
            return (false, AecMutationResult::default());
        };

        let result = entry.election.try_add_fork(fork, fork_tally);
        let mut mutation = AecMutationResult::default();
        let added = match result {
            AddForkResult::Added => {
                mutation
                    .facts
                    .push(AecFact::BlockAddedToElection(fork.hash()));
                true
            }
            AddForkResult::Replaced(removed) => {
                mutation.delta.disconnect_hash(removed.hash());
                mutation.facts.push(AecFact::BlockDiscarded(removed.into()));
                mutation
                    .facts
                    .push(AecFact::BlockAddedToElection(fork.hash()));
                true
            }
            AddForkResult::TallyTooLow => {
                mutation.facts.push(AecFact::BlockDiscarded(fork.clone()));
                false
            }
            AddForkResult::Duplicate | AddForkResult::ElectionEnded => false,
        };

        if added {
            mutation.delta.connect(fork.hash(), fork.qualified_root());
            mutation.delta.conflicts += 1;
        }

        (added, mutation)
    }

    pub fn stop(&mut self) {
        self.roots.clear();
    }

    pub fn is_active_root(&self, root: &QualifiedRoot) -> bool {
        self.roots.get(root).is_some()
    }

    /// Returns the current active elections after transitioning
    pub(crate) fn transition_time(&mut self, now: Timestamp) -> AecMutationResult {
        for entry in self.roots.iter_mut() {
            entry.election.transition_time(now);
        }
        let mut result = self.erase_ended_elections();
        result.delta.ticked += 1;
        result
    }

    pub fn election_for_root(&self, root: &QualifiedRoot) -> Option<&Election> {
        self.roots.election_for_root(root)
    }

    pub fn transition_active(&mut self, root: &QualifiedRoot) -> bool {
        let Some(election) = self.roots.election_for_root_mut(&root) else {
            return false;
        };
        election.transition_active();
        true
    }

    pub fn remove_votes<'a>(
        &mut self,
        root: &QualifiedRoot,
        voters: impl IntoIterator<Item = &'a PublicKey>,
    ) {
        let Some(election) = self.roots.election_for_root_mut(root) else {
            return;
        };
        for voter in voters {
            election.remove_vote(voter);
        }
    }

    pub(crate) fn erase_ended_elections(&mut self) -> AecMutationResult {
        let removed = self.roots.drain_filter(|i| i.election.state().has_ended());
        let mut result = AecMutationResult::default();

        for entry in removed {
            result.delta.election_stopped(&entry.election);
            result.delta.disconnect_election(&entry.election);
            result.facts.push(AecFact::ElectionEnded(entry.election));
        }
        result
    }

    pub(crate) fn erase(&mut self, root: &QualifiedRoot) -> Option<AecMutationResult> {
        let Some(entry) = self.roots.erase(root) else {
            return None;
        };
        let mut result =
            AecMutationResult::from_fact(AecFact::ElectionEnded(entry.election.clone()));
        result.delta.election_stopped(&entry.election);
        result.delta.disconnect_election(&entry.election);
        Some(result)
    }

    pub(crate) fn replace_lowest_priority(
        &mut self,
        root: &QualifiedRoot,
        request: AecInsertRequest,
        now: Timestamp,
    ) -> Result<AecMutationResult, AecInsertError> {
        let Some(erased) = self.roots.erase(root) else {
            return Err(AecInsertError::Duplicate);
        };

        let mut result =
            AecMutationResult::from_fact(AecFact::ElectionEnded(erased.election.clone()));
        result.delta.election_stopped(&erased.election);
        result.delta.disconnect_election(&erased.election);
        result.merge(self.insert_new_election(request, now));
        Ok(result)
    }

    /// Dependent elections are implicitly confirmed when their block is confirmed
    pub(crate) fn confirm_dependent_elections(
        &mut self,
        confirmed: Vec<(SavedBlock, Option<ConfirmedElection>)>,
        now: Timestamp,
    ) -> AecMutationResult {
        let mut result = AecMutationResult::default();
        for (confirmed_block, source_election) in confirmed {
            let confirmed_election =
                self.confirm_dependent_election(&confirmed_block, source_election, now);

            result.delta.block_confirmations[confirmed_election.confirmation_type as usize] += 1;
            result
                .facts
                .push(self.block_confirmed(confirmed_block, confirmed_election));
        }
        result
    }

    fn confirm_dependent_election(
        &mut self,
        confirmed_block: &SavedBlock,
        source_election: Option<ConfirmedElection>,
        now: Timestamp,
    ) -> ConfirmedElection {
        // Check if the currently confirmed block was part of an election that triggered
        // the block confirmation
        if let Some(source) = source_election
            && confirmed_block.hash() == source.winner.hash()
        {
            // This is the block that was directly confirmed by the source election.
            // The election is already confirmed, so there is nothing to do.
            return source;
        }

        let Some(corresponding) = self.roots.get_mut(&confirmed_block.qualified_root()) else {
            return ConfirmedElection::new(
                confirmed_block.clone(),
                ConfirmationType::InactiveConfirmationHeight,
            );
        };

        if corresponding.election.winner().hash() == confirmed_block.hash() {
            corresponding.election.force_confirm();
            corresponding
                .election
                .into_confirmed_election(now, ConfirmationType::ActiveConfirmationHeight)
        } else {
            corresponding.election.cancel();
            ConfirmedElection::new(
                confirmed_block.clone(),
                ConfirmationType::ActiveConfirmationHeight,
            )
        }
    }

    fn block_confirmed(&mut self, block: SavedBlock, election: ConfirmedElection) -> AecFact {
        AecFact::BlockConfirmed(block, election)
    }

    pub(crate) fn apply_vote<'a>(
        &mut self,
        args: ApplyVoteArgs<'a>,
        vote_router: &super::vote_router::VoteRouter,
        was_recently_confirmed: &dyn Fn(&BlockHash) -> bool,
    ) -> ApplyVoteResult {
        let mut apply_helper = ApplyVoteHelper {
            args: &args,
            was_recently_confirmed,
            roots: &mut self.roots,
            vote_router,
        };
        let mut result = apply_helper.apply_vote();
        for entry in std::mem::take(&mut result.confirmed) {
            result.delta.election_stopped(&entry.election);
            result.delta.disconnect_election(&entry.election);
            result.facts.push(AecFact::ElectionEnded(entry.election));
        }
        result
    }

    pub(crate) fn force_confirm(
        &mut self,
        root: &QualifiedRoot,
        now: Timestamp,
    ) -> AecMutationResult {
        let Some(election) = self.roots.election_for_root_mut(root) else {
            panic!("Force confirm failed, because no active election was found");
        };
        if election.force_confirm() {
            let confirmed_election =
                election.into_confirmed_election(now, ConfirmationType::ActiveConfirmedQuorum);
            let mut delta = AecContainerDelta::default();
            delta
                .recently_confirmed
                .push((election.qualified_root().clone(), election.winner().hash()));
            AecMutationResult {
                facts: AecFact::ElectionConfirmed(confirmed_election).into(),
                delta,
            }
        } else {
            AecMutationResult::default()
        }
    }

    pub fn cancel(&mut self, root: &QualifiedRoot) {
        if let Some(entry) = self.roots.get_mut(root) {
            entry.election.cancel();
        }
    }

    pub fn cancel_all(&mut self) {
        for entry in self.roots.iter_mut() {
            entry.election.cancel();
        }
    }

    pub fn len(&self) -> usize {
        self.roots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn info(&self) -> ActiveElectionsInfo {
        ActiveElectionsInfo {
            max_elections: 0,
            total: self.roots.len(),
            priority: 0,
            hinted: 0,
            optimistic: 0,
        }
    }
}

impl Default for ActiveElectionsContainer {
    fn default() -> Self {
        Self::new(Duration::from_secs(1))
    }
}

impl ContainerInfoProvider for ActiveElectionsContainer {
    fn container_info(&self) -> ContainerInfo {
        ContainerInfo::builder()
            .leaf("roots", self.roots.len(), RootContainer::ELEMENT_SIZE)
            .finish()
    }
}

pub struct ApplyVoteArgs<'a> {
    pub vote: &'a FilteredVote,
    pub rep_weights: &'a RepWeights,
    pub quorum_specs: &'a QuorumSpecs,
    pub now: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::ReceivedVote;
    use crate::consensus::active_elections::vote_router::VoteRouter;
    use rsnano_types::{BlockPriority, PrivateKey, TimePriority, Vote, VoteSource};
    use std::sync::Arc;

    #[test]
    fn empty() {
        let container = ActiveElectionsContainer::default();
        assert_eq!(container.len(), 0);
        assert!(!container.is_active_root(&QualifiedRoot::new_test_instance()));
    }

    #[test]
    fn insert_election() {
        let mut container = ActiveElectionsContainer::default();
        let request = AecInsertRequest {
            block: SavedBlock::new_test_instance(),
            behavior: ElectionBehavior::Priority,
            priority: BlockPriority::new_test_instance(),
        };

        let facts = container
            .insert(request, Timestamp::new_test_instance())
            .unwrap();

        assert_eq!(container.len(), 1);
        assert!(matches!(
            facts.facts.as_slice(),
            [AecFact::ElectionStarted(_, _)]
        ));
    }

    #[test]
    fn confirm_election() {
        let mut container = ActiveElectionsContainer::default();

        let block = SavedBlock::new_test_instance();
        let block_hash = block.hash();
        let root = block.qualified_root();

        let request = AecInsertRequest {
            block,
            behavior: ElectionBehavior::Priority,
            priority: BlockPriority::new_test_instance(),
        };

        let now = Timestamp::new_test_instance();
        let insert_facts = container.insert(request, now).unwrap();
        let mut router = VoteRouter::default();
        for (hash, root) in &insert_facts.delta.route_additions {
            router.connect(*hash, root.clone());
        }

        let rep_key = PrivateKey::from(1);
        let received_vote = test_final_vote(&rep_key, block_hash);

        let mut rep_weights = RepWeights::default();
        rep_weights.put(rep_key.public_key(), Amount::MAX);

        let result = container.apply_vote(
            ApplyVoteArgs {
                vote: &received_vote.into(),
                rep_weights: &rep_weights,
                quorum_specs: &QuorumSpecs::new_test_instance(),
                now,
            },
            &router,
            &|_| false,
        );

        assert_eq!(result.per_block.get(&block_hash), Some(&Ok(())));
        assert!(matches!(
            insert_facts.facts.as_slice(),
            [AecFact::ElectionStarted(_, _)]
        ));
        assert!(matches!(
            result.facts.as_slice(),
            [AecFact::ElectionConfirmed(_), AecFact::ElectionEnded(_)]
        ));

        assert!(container.election_for_root(&root).is_none());
    }

    #[test]
    fn iter_round_robin() {
        let block_a = SavedBlock::new_test_instance_with_key(1);
        let block_b = SavedBlock::new_test_instance_with_key(2);
        let block_c = SavedBlock::new_test_instance_with_key(3);
        let block_d = SavedBlock::new_test_instance_with_key(4);

        let prio_a = BlockPriority::new(Amount::nano(1), TimePriority::new(100));
        let prio_b = BlockPriority::new(Amount::nano(100), TimePriority::new(100));
        let prio_c = BlockPriority::new(Amount::nano(100), TimePriority::new(99));
        let prio_d = BlockPriority::new(Amount::nano(1_000_000), TimePriority::new(100));

        test_iter(&[], &[]);

        test_iter(&[(&block_a, prio_a)], &[&block_a]);

        test_iter(
            &[
                (&block_d, prio_d),
                (&block_a, prio_a),
                (&block_c, prio_c),
                (&block_b, prio_b),
            ],
            &[&block_d, &block_c, &block_a, &block_b],
        )
    }

    #[test]
    fn next_vote_to_broadcast_records_last_vote_once() {
        let mut container = ActiveElectionsContainer::default();
        let block = SavedBlock::new_test_instance();
        let now = Timestamp::new_test_instance();
        let interval = Duration::from_secs(30);
        let priority = BlockPriority::new_test_instance();
        let bucket = crate::consensus::election_schedulers::priority::bucket_index(
            ElectionBehavior::Priority,
            priority.balance,
        );

        container
            .insert(
                AecInsertRequest {
                    block: block.clone(),
                    behavior: ElectionBehavior::Priority,
                    priority,
                },
                now,
            )
            .unwrap();

        let first = container.next_vote_to_broadcast(bucket, interval, now);
        let second = container.next_vote_to_broadcast(bucket, interval, now);

        assert_eq!(
            first,
            Some((block.root(), block.hash(), VoteType::NonFinal))
        );
        assert_eq!(second, None);
    }

    #[test]
    fn priority_activation_upgrades_existing_optimistic_election() {
        let mut container = ActiveElectionsContainer::default();
        let block = SavedBlock::new_test_instance();
        let priority = BlockPriority::new_test_instance();
        let now = Timestamp::new_test_instance();
        let bucket_index =
            crate::consensus::election_schedulers::priority::prio_bucket_index(priority.balance);

        container
            .insert(
                AecInsertRequest {
                    block: block.clone(),
                    behavior: ElectionBehavior::Optimistic,
                    priority,
                },
                now,
            )
            .unwrap();

        let facts = container
            .activate(
                AecActivateRequest::priority(block.clone(), priority, bucket_index, 1),
                now,
                false,
                1,
            )
            .unwrap();

        assert_eq!(facts.facts.len(), 0);
        assert_eq!(
            container
                .election_for_root(&block.qualified_root())
                .unwrap()
                .behavior(),
            ElectionBehavior::Priority
        );
    }

    fn test_final_vote(rep_key: &PrivateKey, block_hash: BlockHash) -> ReceivedVote {
        let vote = Arc::new(Vote::new_final(rep_key, vec![block_hash]));
        ReceivedVote::new(vote, VoteSource::Live, None)
    }

    fn test_iter(blocks: &[(&SavedBlock, BlockPriority)], expected: &[&SavedBlock]) {
        let mut container = ActiveElectionsContainer::default();

        for (block, prio) in blocks {
            let request = AecInsertRequest {
                block: (**block).clone(),
                behavior: ElectionBehavior::Priority,
                priority: *prio,
            };

            container
                .insert(request, Timestamp::new_test_instance())
                .unwrap();
        }

        let result: Vec<_> = container
            .iter_round_robin()
            .map(|i| i.winner().hash())
            .collect();
        let expected: Vec<_> = expected.iter().map(|i| i.hash()).collect();
        assert_eq!(result, expected);
    }
}
