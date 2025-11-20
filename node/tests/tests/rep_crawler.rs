use rsnano_ledger::{
    DEV_GENESIS_ACCOUNT, DEV_GENESIS_HASH, DEV_GENESIS_PUB_KEY,
    test_helpers::UnsavedBlockLatticeBuilder,
};
use rsnano_messages::{ConfirmAck, Message};
use rsnano_network::{ChannelMode, TrafficType};
use rsnano_node::consensus::ReceivedVote;
use rsnano_types::{Amount, DEV_GENESIS_KEY, PrivateKey, UnixMillisTimestamp, Vote, VoteSource};
use std::{sync::Arc, time::Duration};
use test_helpers::{System, assert_always_eq, assert_never, assert_timely_eq, assert_timely_eq2};

#[test]
fn ignore_rebroadcast() {
    let mut system = System::new();
    let node1 = system.make_node();
    let node2 = system.make_node();

    let channel1to2 = node1
        .network_subsystem()
        .test_handles()
        .network
        .read()
        .unwrap()
        .find_node_id(&node2.node_id())
        .expect("channel not found 1 to 2")
        .channel_id();

    let channel2to1 = node2
        .network_subsystem()
        .test_handles()
        .network
        .read()
        .unwrap()
        .find_node_id(&node1.node_id())
        .cloned()
        .expect("channel not found 2 to 1");

    node1
        .consensus_subsystem()
        .test_handles()
        .rep_crawler
        .force_query(*DEV_GENESIS_HASH, channel1to2);

    assert_always_eq(
        Duration::from_millis(100),
        || {
            node1
                .consensus_subsystem()
                .test_handles()
                .online_reps
                .lock()
                .unwrap()
                .peered_reps_count()
        },
        0,
    );

    // Now we spam the vote for genesis, so it appears as a rebroadcasted vote
    let vote = Vote::new(
        &DEV_GENESIS_KEY,
        UnixMillisTimestamp::ZERO,
        0,
        vec![*DEV_GENESIS_HASH],
    );
    node1
        .consensus_subsystem()
        .test_handles()
        .rep_crawler
        .force_query(*DEV_GENESIS_HASH, channel1to2);

    let message_sender = node2.network_subsystem().test_handles().message_sender;
    let tick = || {
        let msg = Message::ConfirmAck(ConfirmAck::new_with_rebroadcasted_vote(vote.clone()));
        message_sender
            .lock()
            .unwrap()
            .try_send(&channel2to1, &msg, TrafficType::RepCrawler);
        false
    };

    assert_never(Duration::from_secs(1), || {
        tick()
            || node1
                .consensus_subsystem()
                .test_handles()
                .online_reps
                .lock()
                .unwrap()
                .peered_reps_count()
                > 0
    })
}

#[test]
fn rep_weight() {
    let mut system = System::new();
    let node = system.make_node();
    let node1 = system.make_node();
    let node2 = system.make_node();
    let node3 = system.make_node();
    let key_non_pr = PrivateKey::new();
    let key_pr = PrivateKey::new();
    let amount_pr = Amount::nano(600_000);
    let amount_not_pr = Amount::nano(10_000);

    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let send_non_pr = lattice.genesis().send(&key_non_pr, amount_not_pr);
    let open_no_pr = lattice.account(&key_non_pr).receive(&send_non_pr);
    let send_pr = lattice.genesis().send(&key_pr, amount_pr);
    let open_pr = lattice.account(&key_pr).receive(&send_pr);
    let blocks = [send_non_pr, open_no_pr, send_pr, open_pr];
    node.process_multi(&blocks);
    node1.process_multi(&blocks);
    node2.process_multi(&blocks);
    node3.process_multi(&blocks);
    assert_eq!(
        node.consensus_subsystem()
            .test_handles()
            .online_reps
            .lock()
            .unwrap()
            .online_reps()
            .count(),
        0
    );

    assert_timely_eq2(
        || {
            node.network_subsystem()
                .test_handles()
                .network
                .read()
                .unwrap()
                .count_by_mode(ChannelMode::Realtime)
        },
        3,
    );

    let (channel1, channel2, channel3) = {
        let network_services = node.network_subsystem().test_handles();
        let network = network_services.network.read().unwrap();
        (
            network.find_node_id(&node1.get_node_id()).unwrap().clone(),
            network.find_node_id(&node2.get_node_id()).unwrap().clone(),
            network.find_node_id(&node3.get_node_id()).unwrap().clone(),
        )
    };

    let vote0 = ReceivedVote::new(
        Arc::new(Vote::new(
            &DEV_GENESIS_KEY,
            UnixMillisTimestamp::ZERO,
            0,
            vec![*DEV_GENESIS_HASH],
        )),
        VoteSource::Live,
        Some(channel1.clone()),
    );

    let vote1 = ReceivedVote::new(
        Arc::new(Vote::new(
            &key_non_pr,
            UnixMillisTimestamp::ZERO,
            0,
            vec![*DEV_GENESIS_HASH],
        )),
        VoteSource::Live,
        Some(channel2.clone()),
    );

    let vote2 = ReceivedVote::new(
        Arc::new(Vote::new(
            &key_pr,
            UnixMillisTimestamp::ZERO,
            0,
            vec![*DEV_GENESIS_HASH],
        )),
        VoteSource::Live,
        Some(channel3.clone()),
    );

    node.consensus_subsystem()
        .test_handles()
        .rep_crawler
        .force_process2(vote0);
    node.consensus_subsystem()
        .test_handles()
        .rep_crawler
        .force_process2(vote1);
    node.consensus_subsystem()
        .test_handles()
        .rep_crawler
        .force_process2(vote2);

    assert_timely_eq2(
        || {
            node.consensus_subsystem()
                .test_handles()
                .online_reps
                .lock()
                .unwrap()
                .peered_reps_count()
        },
        2,
    );
    // Make sure we get the rep with the most weight first
    let rep = node
        .consensus_subsystem()
        .test_handles()
        .online_reps
        .lock()
        .unwrap()
        .peered_reps()[0]
        .clone();
    assert_eq!(
        node.balance(&DEV_GENESIS_ACCOUNT),
        node.ledger_query_services().ledger.weight(&rep.rep_key)
    );
    assert_eq!(channel1, rep.channel);
    assert_eq!(
        node.consensus_subsystem()
            .test_handles()
            .online_reps
            .lock()
            .unwrap()
            .is_principal_rep(channel1.channel_id()),
        true
    );
    assert_eq!(
        node.consensus_subsystem()
            .test_handles()
            .online_reps
            .lock()
            .unwrap()
            .is_principal_rep(channel2.channel_id()),
        false
    );
    assert_eq!(
        node.consensus_subsystem()
            .test_handles()
            .online_reps
            .lock()
            .unwrap()
            .is_principal_rep(channel3.channel_id()),
        true
    );
}

// Test that nodes can track nodes that have rep weight for priority broadcasting
#[test]
fn rep_list() {
    let mut system = System::new();
    let node1 = system.make_node();
    let node2 = system.make_node();
    assert_eq!(
        0,
        node2
            .consensus_subsystem()
            .test_handles()
            .online_reps
            .lock()
            .unwrap()
            .peered_reps_count()
    );
    // Node #1 has a rep
    node1.wallet_services().insert_into_wallet(&DEV_GENESIS_KEY);
    assert_timely_eq(
        Duration::from_secs(5),
        || {
            node2
                .consensus_subsystem()
                .test_handles()
                .online_reps
                .lock()
                .unwrap()
                .peered_reps_count()
        },
        1,
    );
    assert_eq!(
        *DEV_GENESIS_PUB_KEY,
        node2
            .consensus_subsystem()
            .test_handles()
            .online_reps
            .lock()
            .unwrap()
            .peered_reps()[0]
            .rep_key
    );
}

#[test]
fn rep_connection_close() {
    let mut system = System::new();
    let node1 = system.make_node();
    let node2 = system.make_node();
    // Add working representative (node 2)
    node2.wallet_services().insert_into_wallet(&DEV_GENESIS_KEY);
    assert_timely_eq(
        Duration::from_secs(10),
        || {
            node1
                .consensus_subsystem()
                .test_handles()
                .online_reps
                .lock()
                .unwrap()
                .peered_reps_count()
        },
        1,
    );
    system.stop_node(node2);
    assert_timely_eq(
        Duration::from_secs(10),
        || {
            node1
                .consensus_subsystem()
                .test_handles()
                .online_reps
                .lock()
                .unwrap()
                .peered_reps_count()
        },
        0,
    );
}

#[test]
fn rep_local() {
    let mut system = System::new();
    let node = system.make_node();
    node.wallet_services().insert_into_wallet(&DEV_GENESIS_KEY);
    assert_timely_eq2(
        || {
            node.consensus_subsystem()
                .test_handles()
                .online_reps
                .lock()
                .unwrap()
                .peered_reps_count()
        },
        1,
    );
}
