use rsnano_network::ChannelMode;
use std::time::Duration;
use test_helpers::{System, assert_never};

// Test a node cannot connect to its own endpoint.
#[test]
fn no_self_incoming() {
    let mut system = System::new();
    let node = system.make_node();
    let network_services = node.network_subsystem().test_handles();
    let _ = network_services
        .peer_connector
        .connect_to(network_services.tcp_listener.local_address());
    assert_never(Duration::from_secs(2), || {
        network_services
            .network
            .read()
            .unwrap()
            .count_by_mode(ChannelMode::Realtime)
            > 0
    })
}
