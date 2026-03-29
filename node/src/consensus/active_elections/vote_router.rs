use std::{collections::HashMap, mem::size_of};

use rsnano_types::{BlockHash, QualifiedRoot};
use rsnano_utils::container_info::ContainerInfo;

use crate::consensus::election::Election;

/// This class routes votes to their associated election
#[derive(Default)]
pub(crate) struct VoteRouter {
    // Mapping of block hashes to elections.
    // Election already contains the associated block
    elections: HashMap<BlockHash, QualifiedRoot>,
}

impl VoteRouter {
    /// Add a route for 'hash' to an election by its qualified root
    /// Existing routes will be replaced
    pub fn connect(&mut self, hash: BlockHash, root: QualifiedRoot) {
        self.elections.insert(hash, root);
    }

    /// Remove all routes to this election
    pub fn disconnect_election(&mut self, election: &Election) {
        for hash in election.candidate_blocks().keys() {
            self.elections.remove(hash);
        }
    }

    /// Remove route to this block
    pub fn disconnect(&mut self, hash: &BlockHash) {
        self.elections.remove(hash);
    }

    pub fn clear(&mut self) {
        self.elections.clear();
    }

    pub fn qualified_root(&self, hash: &BlockHash) -> Option<&QualifiedRoot> {
        self.elections.get(hash)
    }

    pub fn is_active(&self, hash: &BlockHash) -> bool {
        self.elections.contains_key(hash)
    }

    pub fn container_info(&self) -> ContainerInfo {
        [(
            "elections",
            self.elections.len(),
            size_of::<BlockHash>() + size_of::<QualifiedRoot>(),
        )]
        .into()
    }
}
