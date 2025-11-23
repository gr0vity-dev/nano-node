use std::{
    net::{Ipv6Addr, SocketAddrV6},
    thread::sleep,
    time::Duration,
};

use rsnano_messages::{Message, TelemetryAck};
use rsnano_nullable_tcp::get_available_port;
use rsnano_utils::stats::{DetailType, Direction, StatType};
use test_helpers::{System, assert_always_eq, assert_never, assert_timely, assert_timely2};

#[test]
fn invalid_signature() {
    let mut system = System::new();
    let node = system.make_node();

    let mut telemetry = node.telemetry_subsystem().local_telemetry();
    telemetry.block_count = 9999; // Change data so signature is no longer valid
    let node_id = telemetry.node_id;
    let message = Message::TelemetryAck(TelemetryAck(Some(telemetry)));

    let network = node.network_subsystem();
    let endpoint = SocketAddrV6::new(Ipv6Addr::LOCALHOST, get_available_port(), 0, 0);
    network.connect_test_peer(endpoint, Some(node_id));
    network
        .enqueue_inbound(message, endpoint)
        .expect("failed to enqueue telemetry");

    let stats = node.stats_service();
    assert_timely(Duration::from_secs(5), || {
        stats.count(
            StatType::Telemetry,
            DetailType::InvalidSignature,
            Direction::In,
        ) > 0
    });
    assert_never(Duration::from_secs(1), || {
        stats.count(StatType::Telemetry, DetailType::Process, Direction::In) > 0
    });
}

#[test]
fn basic() {
    let mut system = System::new();
    let node_client = system.make_node();
    let node_server = system.make_node();

    let client_network = node_client.network_subsystem();
    let server_endpoint = node_server.network_subsystem().local_endpoint();
    let peer_endpoint = client_network
        .channel_infos()
        .into_iter()
        .find(|info| info.node_id == Some(node_server.get_node_id()))
        .map(|info| info.endpoint)
        .unwrap_or_else(|| {
            client_network.connect_test_peer(server_endpoint, Some(node_server.get_node_id()));
            server_endpoint
        });

    let telemetry = node_client.telemetry_subsystem();

    assert_timely2(|| telemetry.telemetry_for(peer_endpoint).is_some());
    let telemetry_data = telemetry
        .telemetry_for(peer_endpoint)
        .expect("telemetry present");
    assert_eq!(node_server.get_node_id(), telemetry_data.node_id);

    // Check the metrics are correct
    // TODO

    // Call again straight away
    let telemetry_data2 = telemetry
        .telemetry_for(peer_endpoint)
        .expect("telemetry present");

    // Call again straight away
    let telemetry_data3 = telemetry
        .telemetry_for(peer_endpoint)
        .expect("telemetry present");

    // we expect at least one consecutive repeat of telemetry
    assert!(telemetry_data == telemetry_data2 || telemetry_data2 == telemetry_data3);

    // Wait the cache period and check cache is not used
    sleep(Duration::from_secs(3));

    let telemetry_data4 = telemetry
        .telemetry_for(peer_endpoint)
        .expect("telemetry present");

    assert_ne!(telemetry_data, telemetry_data4);
}

#[test]
fn disconnected() {
    let mut system = System::new();
    let node_client = system.make_node();
    let node_server = system.make_node();

    let client_network = node_client.network_subsystem();
    let server_endpoint = node_server.network_subsystem().local_endpoint();
    let peer_endpoint = client_network
        .channel_infos()
        .into_iter()
        .find(|info| info.node_id == Some(node_server.get_node_id()))
        .map(|info| info.endpoint)
        .unwrap_or_else(|| {
            client_network.connect_test_peer(server_endpoint, Some(node_server.get_node_id()));
            server_endpoint
        });

    // Ensure telemetry is available before disconnecting
    let telemetry = node_client.telemetry_subsystem();

    assert_timely(Duration::from_secs(5), || {
        telemetry.telemetry_for(peer_endpoint).is_some()
    });
    system.stop_node(node_server);

    // Ensure telemetry from disconnected peer is removed
    assert_timely(Duration::from_secs(5), || {
        telemetry.telemetry_for(peer_endpoint).is_none()
    });
}

#[test]
fn mismatched_node_id() {
    let mut system = System::new();
    let node = system.make_node();

    let telemetry = node.telemetry_subsystem().local_telemetry();

    let message = Message::TelemetryAck(TelemetryAck(Some(telemetry)));
    let network = node.network_subsystem();
    let endpoint = SocketAddrV6::new(Ipv6Addr::LOCALHOST, get_available_port(), 0, 0);
    network.connect_test_peer(endpoint, None);
    network
        .enqueue_inbound(message, endpoint)
        .expect("failed to enqueue telemetry");

    let stats = node.stats_service();
    assert_timely(Duration::from_secs(5), || {
        stats.count(
            StatType::Telemetry,
            DetailType::NodeIdMismatch,
            Direction::In,
        ) > 0
    });
    assert_always_eq(
        Duration::from_secs(1),
        || stats.count(StatType::Telemetry, DetailType::Process, Direction::In),
        0,
    );
}

#[test]
fn no_peers() {
    let mut system = System::new();
    let node = system.make_node();
    let responses = node.telemetry_subsystem().all_telemetry();
    assert_eq!(responses.len(), 0);
}

#[test]
fn invalid_endpoint() {
    let mut system = System::new();
    let node = system.make_node();
    let endpoint: SocketAddrV6 = "[::ffff:240.0.0.0]:12345".parse().unwrap();
    assert!(node.telemetry_subsystem().telemetry_for(endpoint).is_none());
}

#[test]
fn ongoing_broadcasts() {
    let mut system = System::new();
    let node1 = system.make_node();
    let node2 = system.make_node();

    let stats1 = node1.stats_service();
    let stats2 = node2.stats_service();
    assert_timely(Duration::from_secs(5), || {
        stats1.count(StatType::Telemetry, DetailType::Process, Direction::In) >= 3
    });
    assert_timely(Duration::from_secs(5), || {
        stats2.count(StatType::Telemetry, DetailType::Process, Direction::In) >= 3
    });
}
