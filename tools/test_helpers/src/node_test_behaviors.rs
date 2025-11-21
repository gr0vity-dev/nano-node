use std::{sync::Arc, time::Duration};

use rsnano_node::Node;
use rsnano_types::{BlockHash, Vote, VoteSource};

use crate::{assert_timely_msg, assert_timely2};

/// High level, behavior-oriented helpers for driving a Node in tests.
///
/// These methods wrap the lower-level test handles to express intent like
/// "start an election for this block" or "queue this vote" without exposing
/// the underlying wiring in every test.
pub trait NodeTestBehavior {
    /// Starts an election for the given block hash and waits until it is active.
    /// Panics if the block cannot be found or the election does not appear within
    /// a short timeout.
    fn start_election_for_test(&self, hash: &BlockHash);

    /// Starts multiple elections and optionally forces confirmation.
    fn start_elections_for_test(&self, hashes: &[BlockHash], forced: bool);

    /// Adds blocks to the manual activation queue without transitioning them active.
    fn activate_hashes_for_test(&self, hashes: &[BlockHash]);

    /// Enqueues a vote into the vote processor queue with default channel/filter.
    fn enqueue_vote_for_test(&self, vote: Arc<Vote>, source: VoteSource) -> bool;
}

impl NodeTestBehavior for Node {
    fn start_elections_for_test(&self, hashes: &[BlockHash], forced: bool) {
        for hash in hashes {
            self.start_election_for_test(hash);
            if forced {
                self.force_confirm(hash);
            }
        }
    }

    fn start_election_for_test(&self, hash: &BlockHash) {
        assert_timely_msg(
            Duration::from_secs(2),
            || self.block_exists(hash),
            "block not found before starting election",
        );

        let block = self
            .block(hash)
            .unwrap_or_else(|| panic!("block {hash:?} not retrievable"));

        self.consensus_subsystem()
            .test_handles()
            .election_schedulers
            .add_manual(block.clone());

        // wait for the election to appear
        let root = block.qualified_root();
        assert_timely2(|| self.is_active_root(&root));

        self.consensus_subsystem()
            .test_handles()
            .active
            .write()
            .unwrap()
            .transition_active(&block.hash());
    }

    fn activate_hashes_for_test(&self, hashes: &[BlockHash]) {
        for hash in hashes {
            let block = self
                .block(hash)
                .unwrap_or_else(|| panic!("block {hash:?} not retrievable"));
            self.consensus_subsystem()
                .test_handles()
                .election_schedulers
                .add_manual(block);
        }
    }

    fn enqueue_vote_for_test(&self, vote: Arc<Vote>, source: VoteSource) -> bool {
        self.consensus_subsystem()
            .test_handles()
            .vote_processor_queue
            .enqueue(vote, None, source, None)
    }
}
