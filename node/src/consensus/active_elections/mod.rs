mod active_elections_container;
mod aec_service;
mod apply_vote_helper;
mod cooldown_controller;
mod recently_confirmed_cache;
mod root_container;
mod stats;
mod vote_router;

use std::{collections::HashMap, isize};

use rsnano_types::{Amount, Block, BlockHash, BlockPriority, QualifiedRoot, SavedBlock, VoteError};

use super::{
    ReceivedVote,
    election::{ConfirmedElection, Election, ElectionBehavior},
};
use crate::consensus::election_schedulers::priority::{prio_bucket_count, prio_bucket_index};
pub use active_elections_container::*;
pub use aec_service::AecService;
pub use cooldown_controller::AecCooldownReason;
use root_container::{Entry, RootContainer};

#[derive(Clone, Debug, PartialEq)]
pub struct ActiveElectionsConfig {
    /// Maximum number of simultaneous active elections (AEC size)
    pub max_elections: usize,
    /// Maximum cache size for recently_confirmed
    pub confirmation_cache: usize,
}

impl Default for ActiveElectionsConfig {
    fn default() -> Self {
        Self {
            max_elections: 5000,
            confirmation_cache: 65536,
        }
    }
}

pub enum AecFact {
    ElectionStarted(BlockHash, QualifiedRoot),
    ElectionConfirmed(ConfirmedElection),

    /// Ended ether confirmed or unconfirmed
    ElectionEnded(Election),

    BlockAddedToElection(BlockHash),
    BlockDiscarded(Block),
    BlockConfirmed(SavedBlock, ConfirmedElection),
    /// old winner + new winner block
    WinnerChanged(BlockHash, Block),

    VoteProcessed(
        ReceivedVote,
        Amount,
        HashMap<BlockHash, Result<(), VoteError>>,
    ),
    Recovered,
}

#[derive(Default)]
pub(super) struct ProducedAecFacts(Vec<AecFact>);

impl ProducedAecFacts {
    pub(super) fn push(&mut self, fact: AecFact) {
        self.0.push(fact);
    }

    pub(super) fn extend(&mut self, other: ProducedAecFacts) {
        self.0.extend(other.0);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl IntoIterator for ProducedAecFacts {
    type Item = AecFact;
    type IntoIter = std::vec::IntoIter<AecFact>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum AecInsertError {
    Stopped,
    Duplicate,

    /// This block or a fork got recently confirmed, so there is no need for a new election.
    RecentlyConfirmed,
}

#[derive(Default)]
pub struct ActiveElectionsInfo {
    pub max_elections: usize,
    pub total: usize,
    pub priority: usize,
    pub hinted: usize,
    pub optimistic: usize,
}

pub struct AecInsertRequest {
    pub block: SavedBlock,
    pub behavior: ElectionBehavior,
    pub priority: BlockPriority,
    pub bucket_id: usize,
}

impl AecInsertRequest {
    pub fn new(
        block: SavedBlock,
        behavior: ElectionBehavior,
        priority: BlockPriority,
        bucket_id: usize,
    ) -> Self {
        Self {
            block,
            behavior,
            priority,
            bucket_id,
        }
    }

    pub fn new_hinted(block: SavedBlock, priority: BlockPriority) -> Self {
        Self::new(
            block,
            ElectionBehavior::Hinted,
            priority,
            prio_bucket_count() + 1,
        )
    }

    pub fn new_optimistic(block: SavedBlock, priority: BlockPriority) -> Self {
        Self::new(
            block,
            ElectionBehavior::Optimistic,
            priority,
            prio_bucket_count() + 2,
        )
    }

    pub fn new_manual(block: SavedBlock, priority: BlockPriority) -> Self {
        Self::new(
            block,
            ElectionBehavior::Manual,
            priority,
            prio_bucket_count(),
        )
    }

    pub fn new_priority(block: SavedBlock, priority: BlockPriority) -> Self {
        Self::new(
            block,
            ElectionBehavior::Priority,
            priority,
            prio_bucket_index(priority.balance),
        )
    }
}

const AEC_STAT_KEY: &str = "active_elections";

/// Provides blocks for which an election should be scheduled
pub trait ElectionCandidateSource {
    fn should_schedule(&self, buckets: &[BucketInfo]) -> bool;

    fn gather_candidates(&mut self, buckets: &[BucketInfo], result: &mut Vec<ElectionCandidate>);
}

#[derive(Clone, PartialEq, Eq)]
pub struct BucketInfo {
    /// The lowest priority of all the elections which are currently in the bucket
    pub lowest_priority: BlockPriority,

    /// Number of elections which are currently in this bucket
    pub election_count: usize,

    /// Maximum number of elections in that bucket
    pub max_elections: usize,
}

impl BucketInfo {
    pub fn new(max_elections: usize) -> Self {
        Self {
            lowest_priority: BlockPriority::MIN,
            election_count: 0,
            max_elections,
        }
    }

    pub fn vacancy(&self) -> isize {
        self.max_elections as isize - self.election_count as isize
    }
}

pub struct ElectionCandidate {
    pub bucket_id: usize,
    pub block: SavedBlock,
    pub priority: BlockPriority,
}
