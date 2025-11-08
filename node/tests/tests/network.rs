use std::{ops::Deref, sync::Arc, thread::sleep, time::Duration};

use rsnano_ledger::{
    DEV_GENESIS_ACCOUNT, DEV_GENESIS_HASH, LedgerSet, test_helpers::UnsavedBlockLatticeBuilder,
};
use rsnano_messages::{ConfirmAck, Keepalive, Message, MessageHeader, MessageSerializer};
use rsnano_network::{ChannelMode, TrafficType};
use rsnano_node::{config::NodeConfig, consensus::VoteProcessorConfig};
use rsnano_types::{
    Account, Amount, Block, DEV_GENESIS_KEY, Networks, PrivateKey, ProtocolInfo, Root,
    StateBlockArgs, UnixMillisTimestamp, Vote,
};
use rsnano_utils::stats::{DetailType, Direction, StatType};

use test_helpers::{
    System, assert_always_eq, assert_timely, assert_timely_eq, assert_timely_eq2,
    assert_timely_msg, assert_timely2, establish_tcp, make_fake_channel, start_election,
};

#[test]
fn last_contacted() {
    let mut system = System::new();

    let node0 = system.make_node();

    let mut node1_config = System::default_config();
    node1_config.tcp.max_inbound_connections = 0; // Prevent ephemeral node1->node0 channel repacement with incoming connection
    let node1 = system
        .build_node()
        .config(node1_config)
        .disconnected()
        .finish();

    let channel1 = establish_tcp(&node1, &node0);
    let node0_network_services = node0.network_services();
    let node0_network = node0_network_services.network.clone();
    assert_timely_eq(
        Duration::from_secs(3),
        || {
            node0_network
                .read()
                .unwrap()
                .count_by_mode(ChannelMode::Realtime)
        },
        1,
    );

    // channel0 is the other side of channel1, same connection different endpoint
    let channel0 = node0_network
        .read()
        .unwrap()
        .find_node_id(&node1.node_id())
        .unwrap()
        .clone();

    // check that the endpoints are part of the same connection
    assert_eq!(channel0.local_addr(), channel1.peer_addr());
    assert_eq!(channel1.local_addr(), channel0.peer_addr());

    // capture the state before and ensure the clock ticks at least once
    let timestamp_before_keepalive = channel0.last_activity();
    let stats = node0.stats_service();
    let keepalive_count =
        stats.count(StatType::Message, DetailType::Keepalive, Direction::In);
    let steady_clock = node0_network_services.steady_clock.clone();
    assert_timely_msg(
        Duration::from_secs(3),
        || steady_clock.now() > timestamp_before_keepalive,
        "clock did not advance",
    );

    // send 3 keepalives
    // we need an extra keepalive to handle the race condition between the timestamp set and the counter increment
    // and we need one more keepalive to handle the possibility that there is a keepalive already in flight when we start the crucial part of the test
    // it is possible that there could be multiple keepalives in flight but we assume here that there will be no more than one in flight for the purposes of this test
    let keepalive = Message::Keepalive(Keepalive::default());
    let mut publisher = node0_network_services.message_sender.lock().unwrap();
    publisher.try_send(&channel1, &keepalive, TrafficType::Generic);
    publisher.try_send(&channel1, &keepalive, TrafficType::Generic);
    publisher.try_send(&channel1, &keepalive, TrafficType::Generic);

    assert_timely_msg(
        Duration::from_secs(3),
        || {
            stats.count(StatType::Message, DetailType::Keepalive, Direction::In)
                >= keepalive_count + 3
        },
        "keepalive count",
    );
    assert_eq!(
        node0_network
            .read()
            .unwrap()
            .count_by_mode(ChannelMode::Realtime),
        1
    );
    let timestamp_after_keepalive = channel0.last_activity();
    assert!(timestamp_after_keepalive > timestamp_before_keepalive);
}

#[test]
fn send_discarded_publish() {
    let mut system = System::new();
    let node1 = system.make_node();
    let node2 = system.make_node();

    let block: Block = StateBlockArgs {
        key: &DEV_GENESIS_KEY,
        previous: 2.into(),
        representative: 3.into(),
        balance: Amount::MAX,
        link: 4.into(),
        work: node1.work_generate_dev(Root::from(2)),
    }
    .into();

    node1
        .consensus_services()
        .local_block_broadcaster
        .flood_block_initial(block);

    assert_eq!(node1.latest(&DEV_GENESIS_ACCOUNT), *DEV_GENESIS_HASH);
    assert_eq!(node2.latest(&DEV_GENESIS_ACCOUNT), *DEV_GENESIS_HASH);
    let node2_stats = node2.stats_service();
    assert_timely2(|| {
        node2_stats.count(StatType::Message, DetailType::Publish, Direction::In) != 0
    });
    assert_eq!(node1.latest(&DEV_GENESIS_ACCOUNT), *DEV_GENESIS_HASH);
    assert_eq!(node2.latest(&DEV_GENESIS_ACCOUNT), *DEV_GENESIS_HASH);
}

#[test]
fn receivable_processor_confirm_insufficient_pos() {
    let mut system = System::new();
    let node1 = system.make_node();
    let consensus_services = node1.consensus_services();
    let active = consensus_services.active.clone();

    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let send1 = lattice.genesis().send(Account::ZERO, 1);
    node1.process(send1.clone());

    start_election(&node1, &send1.hash());
    let key1 = PrivateKey::new();
    let vote = Arc::new(Vote::new_final(&key1, vec![send1.hash()]));
    let network_services = node1.network_services();
    let channel = make_fake_channel(&network_services);
    let con1 = Message::ConfirmAck(ConfirmAck::new_with_rebroadcasted_vote(
        vote.deref().clone(),
    ));
    assert_eq!(
        0,
        active
            .read()
            .unwrap()
            .election_for_block(&send1.hash())
            .unwrap()
            .vote_count()
    );

    let inbound_queue = node1.network_services().inbound_message_queue;
    inbound_queue.put(con1, channel);

    assert_timely_eq2(
        || {
            active
                .read()
                .unwrap()
                .election_for_block(&send1.hash())
                .unwrap()
                .vote_count()
        },
        1,
    );
}

#[test]
fn receivable_processor_confirm_sufficient_pos() {
    let mut system = System::new();
    let node1 = system.make_node();
    let consensus_services = node1.consensus_services();
    let active = consensus_services.active.clone();
    let ledger_services = node1.ledger_query_services();

    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let send1 = lattice.genesis().send(Account::ZERO, 1);
    node1.process(send1.clone());

    start_election(&node1, &send1.hash());
    let vote = Arc::new(Vote::new_final(&DEV_GENESIS_KEY, vec![send1.hash()]));
    let network_services = node1.network_services();
    let channel = make_fake_channel(&network_services);
    let con1 = Message::ConfirmAck(ConfirmAck::new_with_rebroadcasted_vote(
        vote.deref().clone(),
    ));
    assert_eq!(
        0,
        active
            .read()
            .unwrap()
            .election_for_block(&send1.hash())
            .unwrap()
            .vote_count()
    );

    let inbound_queue = node1.network_services().inbound_message_queue;
    inbound_queue.put(con1, channel);

    assert_timely2(|| ledger_services.ledger.confirmed().block_exists(&send1.hash()));
}

#[test]
fn multi_keepalive() {
    let mut system = System::new();
    let node1 = system.make_node();
    let _node2 = system.make_node();
    let stats = node1.stats_service();
    assert_timely(Duration::from_secs(10), || {
        stats.count(StatType::Message, DetailType::Keepalive, Direction::In) > 0
    });
}

#[test]
fn send_valid_confirm_ack() {
    let mut system = System::new();
    let node1 = system.make_node();
    let node2 = system.make_node();
    let key2 = PrivateKey::new();
    node1.insert_into_wallet(&DEV_GENESIS_KEY);
    node2.insert_into_wallet(&key2);

    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let block2 = lattice.genesis().send_all_except(&key2, 50);
    node1.process_active(block2);
    // Keep polling until latest block changes
    assert_timely(Duration::from_secs(10), || {
        node2.latest(&DEV_GENESIS_ACCOUNT) != *DEV_GENESIS_HASH
    });
    // Make sure the balance has decreased after processing the block.
    assert_eq!(node2.balance(&DEV_GENESIS_ACCOUNT), Amount::raw(50));
}

#[test]
fn send_valid_publish() {
    let mut system = System::new();
    let node1 = system.make_node();
    let node2 = system.make_node();
    node1.insert_into_wallet(&DEV_GENESIS_KEY);
    let key2 = PrivateKey::new();
    node2.insert_into_wallet(&key2);

    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let block2 = lattice.genesis().send_all_except(&key2, 50);
    let hash2 = block2.hash();
    let latest2 = node2.latest(&DEV_GENESIS_ACCOUNT);
    node2.process_active(block2);
    let node1_stats = node1.stats_service();
    assert_timely(Duration::from_secs(10), || {
        node1_stats.count(StatType::Message, DetailType::Publish, Direction::In) > 0
    });
    assert_ne!(hash2, latest2);
    assert_timely(Duration::from_secs(10), || {
        node2.latest(&DEV_GENESIS_ACCOUNT) != latest2
    });
    assert_eq!(node2.balance(&DEV_GENESIS_ACCOUNT), Amount::raw(50));
}

#[test]
fn send_with_receive() {
    let mut system = System::new();
    let node1 = system.make_node();
    let node2 = system.make_node();
    let key2 = PrivateKey::new();
    node1.insert_into_wallet(&DEV_GENESIS_KEY);

    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let block1 = lattice.genesis().send(&key2, node1.config.receive_minimum);

    node1.process_active(block1.clone());
    assert_timely(Duration::from_secs(5), || {
        node1.block_exists(&block1.hash())
    });
    node2.process_active(block1.clone());
    assert_timely(Duration::from_secs(5), || {
        node2.block_exists(&block1.hash())
    });
    node2.insert_into_wallet(&key2);
    assert_timely(Duration::from_secs(10), || {
        node1.balance(&key2.public_key().as_account()) == node1.config.receive_minimum
            && node2.balance(&key2.public_key().as_account()) == node1.config.receive_minimum
    });
}

#[test]
fn receive_weight_change() {
    let mut system = System::new();
    let node1 = system.make_node();
    let node2 = system.make_node();
    let key2 = PrivateKey::new();
    node1.insert_into_wallet(&DEV_GENESIS_KEY);
    node2.insert_into_wallet(&key2);
    let node1_wallets = node1.wallet_services();
    let node2_wallets = node2.wallet_services();
    node2_wallets
        .wallets
        .set_representative(
            node2_wallets.wallets.wallet_ids()[0],
            key2.public_key(),
            false,
        )
        .wait()
        .unwrap();
    node1_wallets
        .wallets
        .send(
            node1_wallets.wallets.wallet_ids()[0],
            *DEV_GENESIS_ACCOUNT,
            key2.public_key().as_account(),
            node1.config.receive_minimum,
            0.into(),
            true,
            None,
        )
        .wait()
        .unwrap();
    let node1_ledger = node1.ledger_query_services().ledger.clone();
    let node2_ledger = node2.ledger_query_services().ledger.clone();
    assert_timely(Duration::from_secs(10), || {
        node1_ledger.any().weight_exact(key2.public_key()) == node1.config.receive_minimum
            && node2_ledger.any().weight_exact(key2.public_key())
                == node1.config.receive_minimum
    });
}

#[test]
fn duplicate_vote_detection() {
    let mut system = System::new();
    let node0 = system.make_node();
    let node1 = system.make_node();

    let vote = Vote::new(
        &DEV_GENESIS_KEY,
        UnixMillisTimestamp::ZERO,
        0,
        vec![*DEV_GENESIS_HASH],
    );
    let message = Message::ConfirmAck(ConfirmAck::new_with_own_vote(vote));

    // Publish duplicate detection through TCP
    let node0_network = node0.network_services();
    let channel = node0_network
        .network
        .read()
        .unwrap()
        .find_node_id(&node1.node_id())
        .cloned()
        .unwrap();

    let message_sender = node0_network.message_sender.clone();
    let node1_stats = node1.stats_service();
    message_sender
        .lock()
        .unwrap()
        .try_send(&channel, &message, TrafficType::Generic);
    assert_always_eq(
        Duration::from_millis(100),
        || {
            node1_stats.count(
                StatType::Filter,
                DetailType::DuplicateConfirmAckMessage,
                Direction::In,
            )
        },
        0,
    );
    message_sender
        .lock()
        .unwrap()
        .try_send(&channel, &message, TrafficType::Generic);
    assert_timely_eq(
        Duration::from_secs(2),
        || {
            node1_stats.count(
                StatType::Filter,
                DetailType::DuplicateConfirmAckMessage,
                Direction::In,
            )
        },
        1,
    );
}

// Ensures that the filter doesn't filter out votes that could not be queued for processing
#[test]
fn duplicate_revert_vote() {
    let mut system = System::new();
    let node0 = system
        .build_node()
        .config(NodeConfig {
            enable_vote_processor: false, // do not drain queued votes
            vote_processor: VoteProcessorConfig {
                max_non_pr_queue: 1,
                max_pr_queue: 1,
                ..VoteProcessorConfig::new(1)
            },
            ..System::default_config()
        })
        .finish();
    let node1 = system
        .build_node()
        .config(NodeConfig {
            enable_vote_processor: false,
            vote_processor: VoteProcessorConfig {
                max_non_pr_queue: 1,
                max_pr_queue: 1,
                ..VoteProcessorConfig::new(1)
            },
            ..System::default_config()
        })
        .finish();

    let vote1 = Vote::new(
        &DEV_GENESIS_KEY,
        UnixMillisTimestamp::new(1),
        0,
        vec![*DEV_GENESIS_HASH],
    );
    let message1 = Message::ConfirmAck(ConfirmAck::new_with_own_vote(vote1));

    let vote2 = Vote::new(
        &DEV_GENESIS_KEY,
        UnixMillisTimestamp::new(2),
        2,
        vec![*DEV_GENESIS_HASH],
    );
    let message2 = Message::ConfirmAck(ConfirmAck::new_with_own_vote(vote2));

    // Publish duplicate detection through TCP
    let node0_network = node0.network_services();
    let channel = node0_network
        .network
        .read()
        .unwrap()
        .find_node_id(&node1.node_id())
        .cloned()
        .unwrap();

    // First vote should be processed
    let message_sender = node0_network.message_sender.clone();
    let node1_stats = node1.stats_service();
    message_sender
        .lock()
        .unwrap()
        .try_send(&channel, &message1, TrafficType::Vote);
    assert_always_eq(
        Duration::from_millis(100),
        || {
            node1_stats.count(
                StatType::Filter,
                DetailType::DuplicateConfirmAckMessage,
                Direction::In,
            )
        },
        0,
    );

    // Second vote should get dropped from processor queue
    message_sender
        .lock()
        .unwrap()
        .try_send(&channel, &message2, TrafficType::Vote);
    assert_always_eq(
        Duration::from_millis(100),
        || {
            node1_stats.count(
                StatType::Filter,
                DetailType::DuplicateConfirmAckMessage,
                Direction::In,
            )
        },
        0,
    );
    // And the filter should not have it
    sleep(Duration::from_millis(500)); // Give the node time to process the vote

    let mut serializer =
        MessageSerializer::new(ProtocolInfo::default_for(Networks::NanoDevNetwork));
    let msg2_bytes = serializer.serialize(&message2);
    let payload_bytes = &msg2_bytes[MessageHeader::SERIALIZED_SIZE..];
    let network_filter = node1.network_services().network_filter;
    assert_eq!(network_filter.check_message(payload_bytes), false);
}

#[test]
fn expire_duplicate_filter() {
    let mut system = System::new();
    let node0 = system
        .build_node()
        .config(NodeConfig {
            network_duplicate_filter_cutoff: 3, // Expire after 3 seconds
            ..System::default_config()
        })
        .finish();
    let node1 = system
        .build_node()
        .config(NodeConfig {
            network_duplicate_filter_cutoff: 3, // Expire after 3 seconds
            ..System::default_config()
        })
        .finish();

    let vote = Vote::new(
        &DEV_GENESIS_KEY,
        UnixMillisTimestamp::ZERO,
        0,
        vec![*DEV_GENESIS_HASH],
    );
    let message = Message::ConfirmAck(ConfirmAck::new_with_own_vote(vote));

    // Publish duplicate detection through TCP
    let node0_network = node0.network_services();
    let channel = node0_network
        .network
        .read()
        .unwrap()
        .find_node_id(&node1.node_id())
        .cloned()
        .unwrap();

    // Send a vote
    let message_sender = node0_network.message_sender.clone();
    message_sender
        .lock()
        .unwrap()
        .try_send(&channel, &message, TrafficType::Generic);

    let node1_stats = node1.stats_service();
    assert_always_eq(
        Duration::from_millis(100),
        || {
            node1_stats.count(
                StatType::Filter,
                DetailType::DuplicateConfirmAckMessage,
                Direction::In,
            )
        },
        0,
    );

    message_sender
        .lock()
        .unwrap()
        .try_send(&channel, &message, TrafficType::Generic);

    assert_timely_eq(
        Duration::from_secs(2),
        || {
            node1_stats.count(
                StatType::Filter,
                DetailType::DuplicateConfirmAckMessage,
                Direction::In,
            )
        },
        1,
    );

    // The filter should expire the vote after some time
    let mut serializer =
        MessageSerializer::new(ProtocolInfo::default_for(Networks::NanoDevNetwork));
    let msg_bytes = serializer.serialize(&message);
    let payload_bytes = &msg_bytes[MessageHeader::SERIALIZED_SIZE..];
    let node1_network = node1.network_services();
    let network_filter = node1_network.network_filter.clone();
    assert!(network_filter.check_message(&payload_bytes));
    assert_timely(Duration::from_secs(10), || {
        !network_filter.check_message(&payload_bytes)
    });
}
