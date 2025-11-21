use std::time::Duration;

use rsnano_utils::stats::{DetailType, Direction, StatType};
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn stats_clear() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    node.runtime()
        .block_on(async { server.client.stats_clear().await.unwrap() });

    let stats = node.stats_service();
    assert_eq!(
        stats.count(StatType::Ledger, DetailType::Fork, Direction::In),
        0
    );

    assert!(stats.last_reset() <= Duration::from_secs(5));
}
