use std::{sync::Arc, time::Duration};

use rsnano_ledger::test_helpers::UnsavedBlockLatticeBuilder;
use rsnano_node::{config::NodeConfig, consensus::ReceivedVote};
use rsnano_types::{Amount, DEV_GENESIS_KEY, PrivateKey, Vote, VoteSource};
use test_helpers::{System, assert_timely2};

// checks that block cannot be confirmed if there is no enough votes to reach quorum
#[test]
fn quorum_minimum_confirm_fail() {
    let mut system = System::new();
    let config = NodeConfig {
        online_weight_minimum: Amount::MAX,
        ..System::default_config_without_backlog_scan()
    };
    let node1 = system.build_node().config(config).finish();
    let wallet_id = node1.wallets.wallet_ids()[0];
    node1
        .wallets
        .insert_adhoc2(&wallet_id, &DEV_GENESIS_KEY.raw_key(), true)
        .unwrap();

    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let key = PrivateKey::new();
    let send1 = lattice.genesis().send(
        &key,
        Amount::MAX - (node1.online_reps.lock().unwrap().quorum_delta() - Amount::raw(1)),
    );

    node1.process_active(send1.clone());
    assert_timely2(|| node1.is_active_root(&send1.qualified_root()));

    let vote = ReceivedVote::new(
        Arc::new(Vote::new_final(&DEV_GENESIS_KEY, vec![send1.hash()])),
        VoteSource::Live,
        None,
    );
    let _ = node1.vote_processor.vote_blocking(&vote.into());

    // Give the election a chance to confirm
    std::thread::sleep(Duration::from_secs(1));

    // It should not confirm because there should not be enough quorum
    assert_eq!(node1.block_confirmed(&send1.hash()), false);
}

// This test ensures blocks can be confirmed precisely at the quorum minimum
#[test]
fn quorum_minimum_confirm_success() {
    let mut system = System::new();
    let config = NodeConfig {
        online_weight_minimum: Amount::MAX,
        ..System::default_config_without_backlog_scan()
    };
    let node1 = system.build_node().config(config).finish();
    let wallet_id = node1.wallets.wallet_ids()[0];
    node1
        .wallets
        .insert_adhoc2(&wallet_id, &DEV_GENESIS_KEY.raw_key(), true)
        .unwrap();

    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let key1 = PrivateKey::new();

    // Only minimum quorum remains
    let send1 = lattice.genesis().send(
        &key1,
        Amount::MAX - node1.online_reps.lock().unwrap().quorum_delta(),
    );

    node1.process_active(send1.clone());
    assert_timely2(|| node1.is_active_root(&send1.qualified_root()));

    let vote = ReceivedVote::new(
        Arc::new(Vote::new_final(&DEV_GENESIS_KEY, vec![send1.hash()])),
        VoteSource::Live,
        None,
    );
    let _ = node1.vote_processor.vote_blocking(&vote.into());

    assert_timely2(|| node1.block_confirmed(&send1.hash()));
}

#[test]
fn quorum_minimum_flip_fail() {
    let mut system = System::new();
    let config = NodeConfig {
        online_weight_minimum: Amount::MAX,
        ..System::default_config_without_backlog_scan()
    };
    let node1 = system.build_node().config(config).finish();

    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let key1 = PrivateKey::new();
    let send1 = lattice.genesis().send(
        &key1,
        Amount::MAX - (node1.online_reps.lock().unwrap().quorum_delta() - Amount::raw(1)),
    );

    let mut fork_lattice = UnsavedBlockLatticeBuilder::new();
    let key2 = PrivateKey::new();
    let send2 = fork_lattice.genesis().send(
        &key2,
        Amount::MAX - (node1.online_reps.lock().unwrap().quorum_delta() - Amount::raw(1)),
    );

    // Process send1 and wait until its election appears
    node1.process_active(send1.clone());
    assert_timely2(|| node1.is_active_root(&send1.qualified_root()));

    // Process send2 and wait until it is added to the existing election
    node1.process_active(send2.clone());
    assert_timely2(|| node1.is_active_hash(&send2.hash()));

    // Genesis generates a final vote for send2 but it should not be enough to reach quorum
    // due to the online_weight_minimum being so high
    let vote = ReceivedVote::new(
        Arc::new(Vote::new_final(&DEV_GENESIS_KEY, vec![send2.hash()])),
        VoteSource::Live,
        None,
    );
    let _ = node1.vote_processor.vote_blocking(&vote.into());

    // Give the election some time before asserting it is not confirmed
    std::thread::sleep(Duration::from_secs(1));

    assert_eq!(node1.block_confirmed(&send2.hash()), false);
}

#[test]
fn quorum_minimum_flip_success() {
    let mut system = System::new();
    let config = NodeConfig {
        online_weight_minimum: Amount::MAX,
        ..System::default_config_without_backlog_scan()
    };
    let node1 = system.build_node().config(config).finish();

    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let key1 = PrivateKey::new();
    let send1 = lattice.genesis().send(
        &key1,
        Amount::MAX - node1.online_reps.lock().unwrap().quorum_delta(),
    );

    let mut fork_lattice = UnsavedBlockLatticeBuilder::new();
    let key2 = PrivateKey::new();
    let send2 = fork_lattice.genesis().send(
        &key2,
        Amount::MAX - node1.online_reps.lock().unwrap().quorum_delta(),
    );

    // Process send1 and wait until its election appears
    node1.process_active(send1.clone());
    assert_timely2(|| node1.is_active_root(&send1.qualified_root()));

    // Process send2 and wait until it is added to the existing election
    node1.process_active(send2.clone());
    assert_timely2(|| node1.is_active_hash(&send2.hash()));

    // Genesis generates a final vote for send2
    let vote = ReceivedVote::new(
        Arc::new(Vote::new_final(&DEV_GENESIS_KEY, vec![send2.hash()])),
        VoteSource::Live,
        None,
    );
    let _ = node1.vote_processor.vote_blocking(&vote.into());

    // Wait for the election to be confirmed
    assert_timely2(|| node1.block_confirmed(&send2.hash()));
}
