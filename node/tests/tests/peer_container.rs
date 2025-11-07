use rsnano_network::ChannelMode;
use std::time::Duration;
use test_helpers::{System, assert_never};

// Test a node cannot connect to its own endpoint.
#[test]
fn no_self_incoming() {
    let mut system = System::new();
    let node = system.make_node();
    let _ = node
        .services()
        .peer_connector
        .connect_to(node.services().tcp_listener.local_address());
    assert_never(Duration::from_secs(2), || {
        node.services()
            .network
            .read()
            .unwrap()
            .count_by_mode(ChannelMode::Realtime)
            > 0
    })
}
