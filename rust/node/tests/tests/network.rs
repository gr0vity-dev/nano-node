use crate::tests::helpers::assert_timely;

use super::helpers::{assert_timely_eq, establish_tcp, System};
use rsnano_core::{Account, Amount, BlockEnum, StateBlock, DEV_GENESIS_KEY};
use rsnano_ledger::{DEV_GENESIS_ACCOUNT, DEV_GENESIS_HASH};
use rsnano_messages::{Keepalive, Message, Publish};
use rsnano_node::{
    stats::{DetailType, Direction, StatType},
    transport::{BufferDropPolicy, ChannelMode, TrafficType},
};
use std::time::{Duration, SystemTime};

#[test]
fn last_contacted() {
    let mut system = System::new();

    let node0 = system.make_node();

    let mut node1_config = System::default_config();
    node1_config.tcp_incoming_connections_max = 0; // Prevent ephemeral node1->node0 channel repacement with incoming connection
    let node1 = system
        .build_node()
        .config(node1_config)
        .disconnected()
        .finish();

    let channel1 = establish_tcp(&node1, &node0);
    assert_timely_eq(
        Duration::from_secs(3),
        || node0.network.count_by_mode(ChannelMode::Realtime),
        1,
    );

    // channel0 is the other side of channel1, same connection different endpoint
    let channel0 = node0
        .network
        .find_node_id(&node1.node_id.public_key())
        .unwrap();

    // check that the endpoints are part of the same connection
    assert_eq!(channel0.local_addr(), channel1.remote_addr());
    assert_eq!(channel1.local_addr(), channel0.remote_addr());

    // capture the state before and ensure the clock ticks at least once
    let timestamp_before_keepalive = channel0.get_last_packet_received();
    let keepalive_count =
        node0
            .stats
            .count(StatType::Message, DetailType::Keepalive, Direction::In);
    assert_timely(
        Duration::from_secs(3),
        || SystemTime::now() > timestamp_before_keepalive,
        "clock did not advance",
    );

    // send 3 keepalives
    // we need an extra keepalive to handle the race condition between the timestamp set and the counter increment
    // and we need one more keepalive to handle the possibility that there is a keepalive already in flight when we start the crucial part of the test
    // it is possible that there could be multiple keepalives in flight but we assume here that there will be no more than one in flight for the purposes of this test
    let keepalive = Message::Keepalive(Keepalive::default());
    channel1.try_send(
        &keepalive,
        BufferDropPolicy::NoLimiterDrop,
        TrafficType::Generic,
    );
    channel1.try_send(
        &keepalive,
        BufferDropPolicy::NoLimiterDrop,
        TrafficType::Generic,
    );
    channel1.try_send(
        &keepalive,
        BufferDropPolicy::NoLimiterDrop,
        TrafficType::Generic,
    );

    assert_timely(
        Duration::from_secs(3),
        || {
            node0
                .stats
                .count(StatType::Message, DetailType::Keepalive, Direction::In)
                >= keepalive_count + 3
        },
        "keepalive count",
    );
    assert_eq!(node0.network.count_by_mode(ChannelMode::Realtime), 1);
    let timestamp_after_keepalive = channel0.get_last_packet_received();
    assert!(timestamp_after_keepalive > timestamp_before_keepalive);
}

#[test]
#[ignore = "todo"]
fn send_discarded_publish() {
    let mut system = System::new();
    let node1 = system.make_node();
    let node2 = system.make_node();

    let block = BlockEnum::State(StateBlock::new(
        *DEV_GENESIS_ACCOUNT,
        999.into(),
        *DEV_GENESIS_ACCOUNT,
        Amount::MAX - Amount::nano(100),
        Account::from(123).into(),
        &DEV_GENESIS_KEY,
        node1.work_generate_dev((*DEV_GENESIS_HASH).into()),
    ));

    node1
        .network
        .flood_message(&Message::Publish(Publish::new_forward(block)), 1.0);

    assert_eq!(node1.latest(&DEV_GENESIS_ACCOUNT), *DEV_GENESIS_HASH);
    assert_eq!(node2.latest(&DEV_GENESIS_ACCOUNT), *DEV_GENESIS_HASH);
    assert_timely(
        Duration::from_secs(10),
        || {
            node2
                .stats
                .count(StatType::Message, DetailType::Publish, Direction::In)
                != 0
        },
        "no publish received",
    );
    assert_eq!(node1.latest(&DEV_GENESIS_ACCOUNT), *DEV_GENESIS_HASH);
    assert_eq!(node2.latest(&DEV_GENESIS_ACCOUNT), *DEV_GENESIS_HASH);
}
