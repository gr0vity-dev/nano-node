use std::time::Duration;
use test_helpers::{System, assert_never};

// Test a node cannot connect to its own endpoint.
#[test]
fn no_self_incoming() {
    let mut system = System::new();
    let node = system.make_node();
    let network = node.network_subsystem();
    let self_endpoint = network.local_endpoint();

    assert!(network.validate_outbound(self_endpoint).is_err());
    // Exercise the connector path; it should refuse and not create a channel.
    let _ = network.connect(self_endpoint);
    assert_never(Duration::from_secs(2), || network.channel_infos().len() > 0)
}
