use std::time::Duration;

use rsnano_ledger::RepWeights;
use rsnano_nullable_clock::Timestamp;
use rsnano_types::{Block, BlockHash, PublicKey, QualifiedRoot, TimePriority};
use rsnano_utils::container_info::ContainerInfo;

use crate::{
    consensus::{
        election::{AddForkResult, Election, ElectionBehavior, VoteType},
        filtered_vote::FilteredVote,
    },
    representatives::QuorumSpecs,
};

use super::{
    AecInsertError, AecInsertRequest, Entry, RootContainer,
    root_container::{BucketCursor, ElectionHandle, RootedElectionHandle},
    vote_router::VoteRouter,
};

pub struct ActiveElectionsContainer {
    roots: RootContainer,
    vote_router: VoteRouter,
    base_latency: Duration,
}

pub(super) enum InsertResult {
    Inserted {
        hash: BlockHash,
        root: QualifiedRoot,
        behavior: ElectionBehavior,
    },
    Upgraded {
        previous_behavior: ElectionBehavior,
        new_behavior: ElectionBehavior,
    },
}

pub(super) enum ForkChange {
    Added {
        added_hash: BlockHash,
    },
    Replaced {
        added_hash: BlockHash,
        removed: Block,
    },
    Discarded {
        discarded: Block,
    },
    Ignored,
}

impl ActiveElectionsContainer {
    pub fn new(base_latency: Duration) -> Self {
        Self {
            roots: RootContainer::default(),
            vote_router: VoteRouter::default(),
            base_latency,
        }
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

    pub fn iter_round_robin(&self) -> impl Iterator<Item = Election> {
        self.roots.iter().map(|i| i.election.snapshot())
    }

    pub fn iter_bucket(&self, bucket_id: usize) -> impl Iterator<Item = Election> {
        self.roots
            .iter_bucket(bucket_id)
            .map(|i| i.election.snapshot())
    }

    pub(super) fn next_bucket(
        &self,
        bucket_id: usize,
        after: Option<&BucketCursor>,
    ) -> Option<(BucketCursor, RootedElectionHandle)> {
        self.roots.next_bucket(bucket_id, after)
    }

    pub(super) fn insert(
        &mut self,
        request: AecInsertRequest,
        now: Timestamp,
    ) -> Result<InsertResult, AecInsertError> {
        if let Some(previous_behavior) = self.try_upgrade_priority_election(&request)? {
            return Ok(InsertResult::Upgraded {
                previous_behavior,
                new_behavior: request.behavior,
            });
        }

        Ok(self.insert_new_election(request, now))
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
        entry.election.lock().voted(vote_type, timestamp);
    }

    fn try_upgrade_priority_election(
        &mut self,
        request: &AecInsertRequest,
    ) -> Result<Option<ElectionBehavior>, AecInsertError> {
        let (upgraded, previous_behavior) = self.roots.try_upgrade_to_priority_election(request);

        if upgraded {
            Ok(previous_behavior)
        } else if previous_behavior.is_some() {
            Err(AecInsertError::Duplicate)
        } else {
            Ok(None)
        }
    }

    fn insert_new_election(&mut self, request: AecInsertRequest, now: Timestamp) -> InsertResult {
        let root = request.block.qualified_root();
        let hash = request.block.hash();
        let election = Election::new(request.block, request.behavior, self.base_latency, now);

        self.roots.insert(Entry {
            root: root.clone(),
            election: ElectionHandle::new(election),
            priority: request.priority,
        });
        self.vote_router.connect(hash, root.clone());
        InsertResult::Inserted {
            hash,
            root,
            behavior: request.behavior,
        }
    }

    pub(super) fn apply_fork_result(
        &mut self,
        root: &QualifiedRoot,
        handle: &ElectionHandle,
        fork: &Block,
        result: AddForkResult,
    ) -> ForkChange {
        let Some(current_handle) = self.roots.election_handle_for_root(root) else {
            return ForkChange::Ignored;
        };
        if !current_handle.ptr_eq(handle) {
            return ForkChange::Ignored;
        }

        match result {
            AddForkResult::Added => {
                self.vote_router.connect(fork.hash(), fork.qualified_root());
                ForkChange::Added {
                    added_hash: fork.hash(),
                }
            }
            AddForkResult::Replaced(removed) => {
                self.vote_router.disconnect(&removed.hash());
                self.vote_router.connect(fork.hash(), fork.qualified_root());
                ForkChange::Replaced {
                    added_hash: fork.hash(),
                    removed: removed.into(),
                }
            }
            AddForkResult::TallyTooLow => ForkChange::Discarded {
                discarded: fork.clone(),
            },
            AddForkResult::Duplicate | AddForkResult::ElectionEnded => ForkChange::Ignored,
        }
    }

    pub fn stop(&mut self) -> Vec<Election> {
        let removed = self
            .roots
            .drain_filter(|_| true)
            .into_iter()
            .map(|entry| entry.election.snapshot())
            .collect();
        self.vote_router.clear();
        removed
    }

    pub fn is_active_root(&self, root: &QualifiedRoot) -> bool {
        self.roots.get(root).is_some()
    }

    pub fn is_active_hash(&self, block_hash: &BlockHash) -> bool {
        self.vote_router.is_active(block_hash)
    }

    pub fn election_for_root(&self, root: &QualifiedRoot) -> Option<Election> {
        self.roots.election_for_root(root)
    }

    pub fn election_for_block(&self, block_hash: &BlockHash) -> Option<Election> {
        let root = self.vote_router.qualified_root(block_hash)?;
        self.election_for_root(root)
    }

    pub fn transition_active(&mut self, block_hash: &BlockHash) -> bool {
        let Some(handle) = self.election_handle_for_block(block_hash) else {
            return false;
        };
        handle.lock().transition_active();
        true
    }

    pub fn remove_votes<'a>(
        &mut self,
        root: &QualifiedRoot,
        voters: impl IntoIterator<Item = &'a PublicKey>,
    ) {
        let Some(handle) = self.roots.election_handle_for_root(root) else {
            return;
        };
        let mut election = handle.lock();
        for voter in voters {
            election.remove_vote(voter);
        }
    }

    pub fn take_ended_elections(&mut self) -> Vec<Election> {
        self.roots
            .drain_filter(|i| i.election.lock().state().has_ended())
            .into_iter()
            .map(|entry| self.cleanup_snapshot(entry.election.snapshot()))
            .collect()
    }

    pub fn erase(&mut self, root: &QualifiedRoot) -> Option<Election> {
        self.roots
            .erase(root)
            .map(|entry| self.cleanup_snapshot(entry.election.snapshot()))
    }

    pub fn erase_lowest_prio_election(&mut self, bucket_id: usize) -> Option<Election> {
        let (root, _) = self.lowest_priority(bucket_id)?;
        self.erase(&root)
    }

    pub(super) fn election_handle_for_block(
        &self,
        block_hash: &BlockHash,
    ) -> Option<ElectionHandle> {
        let root = self.vote_router.qualified_root(block_hash)?;
        self.roots.election_handle_for_root(root)
    }

    pub(super) fn election_handle_for_root(&self, root: &QualifiedRoot) -> Option<ElectionHandle> {
        self.roots.election_handle_for_root(root)
    }

    pub(super) fn erase_with_known_election(
        &mut self,
        root: &QualifiedRoot,
        election: &Election,
    ) -> bool {
        if self.roots.erase_with_known_election(root, election).is_some() {
            self.cleanup_snapshot(election.clone());
            true
        } else {
            false
        }
    }

    pub fn cancel(&mut self, root: &QualifiedRoot) {
        if let Some(handle) = self.roots.election_handle_for_root(root) {
            handle.lock().cancel();
        }
    }

    pub fn cancel_all(&mut self) {
        for entry in self.roots.iter() {
            entry.election.lock().cancel();
        }
    }

    pub fn len(&self) -> usize {
        self.roots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn vote_router_container_info(&self) -> ContainerInfo {
        self.vote_router.container_info()
    }

    fn cleanup_snapshot(&mut self, election: Election) -> Election {
        self.vote_router.disconnect_election(&election);
        election
    }
}

impl Default for ActiveElectionsContainer {
    fn default() -> Self {
        Self::new(Duration::from_secs(1))
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
    use crate::consensus::{AecService, ReceivedVote};
    use rsnano_types::{Amount, BlockPriority, PrivateKey, SavedBlock, TimePriority, Vote, VoteSource};
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
        let block = SavedBlock::new_test_instance();
        let request = AecInsertRequest {
            block: block.clone(),
            behavior: ElectionBehavior::Priority,
            priority: BlockPriority::new_test_instance(),
        };

        container
            .insert(request, Timestamp::new_test_instance())
            .unwrap();

        assert_eq!(container.len(), 1);
        assert!(container.is_active_hash(&block.hash()));
        assert_eq!(
            container
                .election_for_block(&block.hash())
                .unwrap()
                .winner()
                .hash(),
            block.hash()
        );
    }

    #[test]
    fn erase_removes_block_route() {
        let mut container = ActiveElectionsContainer::default();
        let block = SavedBlock::new_test_instance();

        container
            .insert(
                AecInsertRequest::new_priority(block.clone(), BlockPriority::new_test_instance()),
                Timestamp::new_test_instance(),
            )
            .unwrap();

        assert!(container.erase(&block.qualified_root()).is_some());
        assert!(!container.is_active_hash(&block.hash()));
        assert!(container.election_for_block(&block.hash()).is_none());
    }

    #[test]
    fn stop_clears_block_routes() {
        let mut container = ActiveElectionsContainer::default();
        let block = SavedBlock::new_test_instance();

        container
            .insert(
                AecInsertRequest::new_priority(block.clone(), BlockPriority::new_test_instance()),
                Timestamp::new_test_instance(),
            )
            .unwrap();

        container.stop();

        assert!(!container.is_active_hash(&block.hash()));
        assert!(container.election_for_block(&block.hash()).is_none());
    }

    #[test]
    fn confirm_election() {
        let block = SavedBlock::new_test_instance();
        let block_hash = block.hash();

        let now = Timestamp::new_test_instance();
        let aec = AecService::new_null();
        aec.insert(
            AecInsertRequest {
                block,
                behavior: ElectionBehavior::Priority,
                priority: BlockPriority::new_test_instance(),
            },
            now,
        )
        .unwrap();

        let rep_key = PrivateKey::from(1);
        let received_vote = test_final_vote(&rep_key, block_hash);

        let mut rep_weights = RepWeights::default();
        rep_weights.put(rep_key.public_key(), Amount::MAX);

        let vote: crate::consensus::FilteredVote = received_vote.into();
        let result = aec.apply_vote(ApplyVoteArgs {
            vote: &vote,
            rep_weights: &rep_weights,
            quorum_specs: &QuorumSpecs::new_test_instance(),
            now,
        });

        assert_eq!(result.get(&block_hash), Some(&Ok(())));
        assert!(aec.election_for_block(&block_hash).is_none());
        assert!(!aec.is_active_hash(&block_hash));
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
