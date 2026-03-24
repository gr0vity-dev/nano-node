use std::time::Duration;

use strum::EnumCount;

use rsnano_ledger::RepWeights;
use rsnano_nullable_clock::Timestamp;
use rsnano_types::{Amount, Block, BlockHash, PublicKey, QualifiedRoot, SavedBlock, TimePriority};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{StatsCollection, StatsSource},
};

use crate::{
    consensus::{
        election::{
            AddForkResult, ConfirmationType, ConfirmedElection, Election, ElectionBehavior,
            VoteType,
        },
        filtered_vote::FilteredVote,
    },
    representatives::QuorumSpecs,
};

use super::{
    ActiveElectionsConfig, ActiveElectionsInfo, AecContainerChange, AecEvent, AecInsertError,
    AecInsertRequest, Entry, RootContainer,
    apply_vote_helper::{ApplyVoteHelper, ApplyVoteResult},
    cooldown_controller::{AecCooldownReason, CooldownController, CooldownResult},
    recently_confirmed_cache::RecentlyConfirmedCache,
    stats::AecStats,
};

pub struct ActiveElectionsContainer {
    roots: RootContainer,
    stopped: bool,
    count_by_behavior: [usize; ElectionBehavior::COUNT],
    base_latency: Duration,
    recently_confirmed: RecentlyConfirmedCache,
    cooldown: CooldownController,
    max_elections: usize,
    stats: AecStats,
}

impl ActiveElectionsContainer {
    pub fn new(config: ActiveElectionsConfig, base_latency: Duration) -> Self {
        Self {
            roots: RootContainer::default(),
            stopped: false,
            count_by_behavior: Default::default(),
            base_latency,
            recently_confirmed: RecentlyConfirmedCache::new(config.confirmation_cache),
            cooldown: CooldownController::default(),
            max_elections: config.max_elections,
            stats: Default::default(),
        }
    }

    pub fn max_len(&self) -> usize {
        self.max_elections
    }

    pub fn count_by_behavior(&self, behavior: ElectionBehavior) -> usize {
        self.count_by_behavior[behavior as usize]
    }

    fn count_by_behavior_mut(&mut self, behavior: ElectionBehavior) -> &mut usize {
        &mut self.count_by_behavior[behavior as usize]
    }

    pub fn bucket_len(&self, bucket_id: usize) -> usize {
        self.roots.bucket_len(bucket_id)
    }

    pub fn find_bucket(&self, root: &QualifiedRoot) -> Option<usize> {
        self.roots.find_bucket(root)
    }

    pub fn lowest_priority(&self, bucket_id: usize) -> Option<(QualifiedRoot, TimePriority)> {
        self.roots.lowest_priority(bucket_id)
    }

    pub fn iter_round_robin(&self) -> impl Iterator<Item = &Election> {
        self.roots.iter().map(|i| &i.election)
    }

    pub fn iter_bucket(&self, bucket_id: usize) -> impl Iterator<Item = &Election> {
        self.roots.iter_bucket(bucket_id).map(|i| &i.election)
    }

    pub fn insert(
        &mut self,
        request: AecInsertRequest,
        now: Timestamp,
    ) -> Result<AecContainerChange<()>, AecInsertError> {
        self.ensure_not_stopped()?;
        self.ensure_not_recently_confirmed(&request)?;

        if self.try_upgrade_priority_election(&request)? {
            return Ok(AecContainerChange::new(()));
        }

        let event = self.insert_new_election(request, now);
        Ok(AecContainerChange::with_events((), vec![event]))
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

    fn ensure_not_stopped(&self) -> Result<(), AecInsertError> {
        if self.stopped {
            Err(AecInsertError::Stopped)
        } else {
            Ok(())
        }
    }

    fn ensure_not_recently_confirmed(
        &self,
        request: &AecInsertRequest,
    ) -> Result<(), AecInsertError> {
        let root = request.block.qualified_root();

        if self.recently_confirmed.root_exists(&root) {
            return Err(AecInsertError::RecentlyConfirmed);
        }
        Ok(())
    }

    fn try_upgrade_priority_election(
        &mut self,
        request: &AecInsertRequest,
    ) -> Result<bool, AecInsertError> {
        let (upgraded, previous_behavior) = self.roots.try_upgrade_to_priority_election(request);

        if upgraded {
            *self.count_by_behavior_mut(previous_behavior.unwrap()) -= 1;
            *self.count_by_behavior_mut(request.behavior) += 1;
            Ok(true)
        } else if previous_behavior.is_some() {
            Err(AecInsertError::Duplicate)
        } else {
            Ok(false)
        }
    }

    fn insert_new_election(&mut self, request: AecInsertRequest, now: Timestamp) -> AecEvent {
        let root = request.block.qualified_root();
        let hash = request.block.hash();
        let election = Election::new(request.block, request.behavior, self.base_latency, now);

        self.roots.insert(Entry {
            root: root.clone(),
            election,
            priority: request.priority,
        });

        *self.count_by_behavior_mut(request.behavior) += 1;
        self.stats.started(request.behavior);
        AecEvent::ElectionStarted(hash, root)
    }

    pub fn try_add_fork(&mut self, fork: &Block, fork_tally: Amount) -> AecContainerChange<bool> {
        let Some(entry) = self.roots.get_mut(&fork.qualified_root()) else {
            return AecContainerChange::new(false);
        };

        let result = entry.election.try_add_fork(fork, fork_tally);
        let mut events = Vec::new();
        let added = match result {
            AddForkResult::Added => {
                events.push(AecEvent::BlockAddedToElection(fork.hash()));
                true
            }
            AddForkResult::Replaced(removed) => {
                self.roots.vote_router.disconnect(&removed.hash());
                events.push(AecEvent::BlockDiscarded(removed.into()));
                events.push(AecEvent::BlockAddedToElection(fork.hash()));
                true
            }
            AddForkResult::TallyTooLow => {
                events.push(AecEvent::BlockDiscarded(fork.clone()));
                false
            }
            AddForkResult::Duplicate | AddForkResult::ElectionEnded => false,
        };

        if added {
            self.roots
                .vote_router
                .connect(fork.hash(), fork.qualified_root());
            self.stats.conflicts += 1;
        }

        AecContainerChange::with_events(added, events)
    }

    /// How many election slots are available
    /// This is a soft limit and can be negative!
    pub fn vacancy(&self) -> i64 {
        if self.cooldown.is_cooling_down() {
            return 0;
        }
        let current_size = self.roots.len() as i64;
        self.max_elections as i64 - current_size
    }

    pub fn set_cooldown(&mut self, cool_down: bool, reason: AecCooldownReason) -> AecContainerChange<()> {
        let result = self.cooldown.set_cooldown(cool_down, reason);
        if result == CooldownResult::Recovered {
            AecContainerChange::with_events((), vec![AecEvent::Recovered])
        } else {
            AecContainerChange::new(())
        }
    }

    pub fn stop(&mut self) {
        self.stopped = true;
        self.roots.clear();
    }

    pub fn is_active_root(&self, root: &QualifiedRoot) -> bool {
        self.roots.get(root).is_some()
    }

    pub fn is_active_hash(&self, block_hash: &BlockHash) -> bool {
        self.roots.vote_router.is_active(block_hash)
    }

    pub fn was_recently_confirmed(&self, block_hash: &BlockHash) -> bool {
        self.recently_confirmed.hash_exists(block_hash)
    }

    pub fn clear_recently_confirmed(&mut self) {
        self.recently_confirmed.clear();
    }

    /// Returns the current active elections after transitioning
    pub fn transition_time(&mut self, now: Timestamp) -> Vec<AecEvent> {
        self.stats.ticked += 1;
        for entry in self.roots.iter_mut() {
            entry.election.transition_time(now);
        }
        self.erase_ended_elections()
    }

    pub fn election_for_root(&self, root: &QualifiedRoot) -> Option<&Election> {
        self.roots.election_for_root(root)
    }

    pub fn election_for_block(&self, block_hash: &BlockHash) -> Option<&Election> {
        self.roots.election_for_block(block_hash)
    }

    pub fn transition_active(&mut self, block_hash: &BlockHash) -> bool {
        let Some(election) = self.roots.election_for_block_mut(block_hash) else {
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

    pub fn erase_ended_elections(&mut self) -> Vec<AecEvent> {
        let removed = self.roots.drain_filter(|i| i.election.state().has_ended());
        let mut events = Vec::new();

        for entry in removed {
            events.push(self.cleanup_election(entry));
        }
        events
    }

    pub fn erase(&mut self, root: &QualifiedRoot) -> AecContainerChange<bool> {
        let Some(entry) = self.roots.erase(root) else {
            return AecContainerChange::new(false);
        };
        AecContainerChange::with_events(true, vec![self.cleanup_election(entry)])
    }

    pub fn erase_lowest_prio_election(&mut self, bucket_id: usize) -> AecContainerChange<bool> {
        let Some((root, _)) = self.lowest_priority(bucket_id) else {
            return AecContainerChange::new(false);
        };
        self.erase(&root)
    }

    fn cleanup_election(&mut self, entry: Entry) -> AecEvent {
        let election = &entry.election;

        // Keep track of election count by election type
        *self.count_by_behavior_mut(election.behavior()) -= 1;

        self.stats.stopped(&entry.election);
        AecEvent::ElectionEnded(entry.election)
    }

    /// Dependent elections are implicitly confirmed when their block is confirmed
    pub fn confirm_dependent_elections(
        &mut self,
        confirmed: Vec<(SavedBlock, Option<ConfirmedElection>)>,
        now: Timestamp,
    ) -> AecContainerChange<()> {
        let mut events = Vec::new();
        for (confirmed_block, source_election) in confirmed {
            let confirmed_election =
                self.confirm_dependent_election(&confirmed_block, source_election, now);

            events.push(self.block_confirmed(confirmed_block, confirmed_election));
        }
        AecContainerChange::with_events((), events)
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

    fn block_confirmed(&mut self, block: SavedBlock, election: ConfirmedElection) -> AecEvent {
        self.stats.block_confirmations[election.confirmation_type as usize] += 1;
        AecEvent::BlockConfirmed(block, election)
    }

    pub fn remove_recently_confirmed(&mut self, block_hash: &BlockHash) {
        self.recently_confirmed.erase(block_hash);
    }

    pub fn apply_vote<'a>(&mut self, args: ApplyVoteArgs<'a>) -> ApplyVoteResult {
        let mut apply_helper = ApplyVoteHelper {
            args: &args,
            recently_confirmed: &mut self.recently_confirmed,
            vote_counter: &mut self.stats.vote_counter,
            roots: &mut self.roots,
        };
        let mut result = apply_helper.apply_vote();
        let mut cleanup_events = Vec::with_capacity(result.confirmed.len());
        for entry in result.confirmed.drain(..) {
            cleanup_events.push(self.cleanup_election(entry));
        }
        cleanup_events.extend(result.events);
        result.events = cleanup_events;
        result
    }

    pub fn force_confirm(&mut self, block_hash: &BlockHash, now: Timestamp) -> AecContainerChange<()> {
        let Some(election) = self.roots.election_for_block_mut(block_hash) else {
            panic!("Force confirm failed, because no active election was found");
        };
        if election.force_confirm() {
            let confirmed_election =
                election.into_confirmed_election(now, ConfirmationType::ActiveConfirmedQuorum);
            AecContainerChange::with_events((), vec![AecEvent::ElectionConfirmed(confirmed_election)])
        } else {
            AecContainerChange::new(())
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
            max_elections: self.max_elections,
            total: self.roots.len(),
            priority: self.count_by_behavior(ElectionBehavior::Priority),
            hinted: self.count_by_behavior(ElectionBehavior::Hinted),
            optimistic: self.count_by_behavior(ElectionBehavior::Optimistic),
        }
    }
}

impl Default for ActiveElectionsContainer {
    fn default() -> Self {
        Self::new(ActiveElectionsConfig::default(), Duration::from_secs(1))
    }
}

impl StatsSource for ActiveElectionsContainer {
    fn collect_stats(&self, result: &mut StatsCollection) {
        self.cooldown.collect_stats(result);
        self.stats.collect_stats(result);
    }
}

impl ContainerInfoProvider for ActiveElectionsContainer {
    fn container_info(&self) -> ContainerInfo {
        ContainerInfo::builder()
            .leaf("roots", self.roots.len(), RootContainer::ELEMENT_SIZE)
            .leaf(
                "normal",
                self.count_by_behavior(ElectionBehavior::Priority),
                0,
            )
            .leaf(
                "hinted".to_string(),
                self.count_by_behavior(ElectionBehavior::Hinted),
                0,
            )
            .leaf(
                "optimistic".to_string(),
                self.count_by_behavior(ElectionBehavior::Optimistic),
                0,
            )
            .node(
                "recently_confirmed",
                self.recently_confirmed.container_info(),
            )
            .node("vote_router", self.roots.vote_router.container_info())
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
    use rsnano_types::{BlockPriority, PrivateKey, TimePriority, Vote, VoteSource};
    use std::sync::Arc;

    #[test]
    fn empty() {
        let container = ActiveElectionsContainer::default();
        assert_eq!(container.len(), 0);
        assert!(!container.is_active_root(&QualifiedRoot::new_test_instance()));
        assert!(!container.is_active_hash(&BlockHash::from(1)));
    }

    #[test]
    fn insert_election() {
        let mut container = ActiveElectionsContainer::default();
        let request = AecInsertRequest {
            block: SavedBlock::new_test_instance(),
            behavior: ElectionBehavior::Priority,
            priority: BlockPriority::new_test_instance(),
        };

        container
            .insert(request, Timestamp::new_test_instance())
            .unwrap();

        assert_eq!(container.len(), 1);
    }

    #[test]
    fn confirm_election() {
        let mut container = ActiveElectionsContainer::default();

        let block = SavedBlock::new_test_instance();
        let block_hash = block.hash();

        let request = AecInsertRequest {
            block,
            behavior: ElectionBehavior::Priority,
            priority: BlockPriority::new_test_instance(),
        };

        let now = Timestamp::new_test_instance();
        container.insert(request, now).unwrap();

        let rep_key = PrivateKey::from(1);
        let received_vote = test_final_vote(&rep_key, block_hash);

        let mut rep_weights = RepWeights::default();
        rep_weights.put(rep_key.public_key(), Amount::MAX);

        let result = container.apply_vote(ApplyVoteArgs {
            vote: &received_vote.into(),
            rep_weights: &rep_weights,
            quorum_specs: &QuorumSpecs::new_test_instance(),
            now,
        });

        assert_eq!(result.per_block.get(&block_hash), Some(&Ok(())));
        assert!(matches!(
            result.events.as_slice(),
            [AecEvent::ElectionEnded(_), AecEvent::ElectionConfirmed(_)]
        ));

        assert!(container.election_for_block(&block_hash).is_none());
    }

    #[test]
    fn apply_vote_returns_confirmation_and_cleanup_events() {
        let mut container = ActiveElectionsContainer::default();

        let block = SavedBlock::new_test_instance();
        let block_hash = block.hash();
        let now = Timestamp::new_test_instance();

        let insert_result = container
            .insert(
                AecInsertRequest::new_priority(block, BlockPriority::new_test_instance()),
                now,
            )
            .unwrap();

        assert!(matches!(
            insert_result.events.as_slice(),
            [AecEvent::ElectionStarted(_, _)]
        ));

        let rep_key = PrivateKey::from(1);
        let received_vote = test_final_vote(&rep_key, block_hash);
        let mut rep_weights = RepWeights::default();
        rep_weights.put(rep_key.public_key(), Amount::MAX);

        let result = container.apply_vote(ApplyVoteArgs {
            vote: &received_vote.into(),
            rep_weights: &rep_weights,
            quorum_specs: &QuorumSpecs::new_test_instance(),
            now,
        });

        assert!(matches!(
            result.events.as_slice(),
            [AecEvent::ElectionEnded(_), AecEvent::ElectionConfirmed(_)]
        ));
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

    fn test_final_vote(rep_key: &PrivateKey, block_hash: BlockHash) -> ReceivedVote {
        let vote = Arc::new(Vote::new_final(rep_key, vec![block_hash]));
        ReceivedVote::new(vote, VoteSource::Live, None)
    }

    fn test_iter(blocks: &[(&SavedBlock, BlockPriority)], expected: &[&SavedBlock]) {
        let mut container = ActiveElectionsContainer::default();

        for (block, prio) in blocks {
            let request = AecInsertRequest::new_priority((**block).clone(), *prio);

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
