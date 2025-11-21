use std::sync::Arc;

use rsnano_types::{DEV_GENESIS_HASH, DEV_GENESIS_KEY, Vote, VoteSource};
use test_helpers::{NodeTestBehavior, System, setup_chain};

#[test]
fn start_election_helper_brings_block_active() {
    let mut system = System::new();
    let node = system.make_node();
    let blocks = setup_chain(&node, 1, &DEV_GENESIS_KEY, false);
    let block = &blocks[0];

    node.start_election_for_test(&block.hash());

    assert!(node.is_active_root(&block.qualified_root()));
}

#[test]
fn enqueue_vote_helper_queues_vote() {
    let mut system = System::new();
    let node = system.make_node();
    let queue = node
        .consensus_subsystem()
        .test_handles()
        .vote_processor_queue
        .clone();

    let vote = Arc::new(Vote::new(
        &DEV_GENESIS_KEY,
        Vote::TIMESTAMP_MIN,
        0,
        vec![*DEV_GENESIS_HASH],
    ));

    assert!(queue.is_empty());
    assert!(node.enqueue_vote_for_test(vote, VoteSource::Live));
    assert!(!queue.is_empty());
}
