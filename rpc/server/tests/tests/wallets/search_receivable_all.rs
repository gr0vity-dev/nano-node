use rsnano_ledger::test_helpers::UnsavedBlockLatticeBuilder;
use rsnano_node::Node;
use rsnano_types::{Amount, DEV_GENESIS_KEY};
use std::sync::Arc;
use test_helpers::{System, assert_timely_eq2, setup_rpc_client_and_server};

#[test]
fn search_receivable_all() {
    let mut system = System::new();
    let node: Arc<Node> = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    node.wallet_services().insert_into_wallet(&DEV_GENESIS_KEY);

    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let send = lattice
        .genesis()
        .send(&*DEV_GENESIS_KEY, node.config().receive_minimum);

    node.process(send);

    node.runtime().block_on(async {
        server.client.search_receivable_all().await.unwrap();
    });

    assert_timely_eq2(|| node.balance(&DEV_GENESIS_KEY.account()), Amount::MAX);
}

#[test]
fn search_receivable_all_fails_without_enable_control() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let result = node
        .runtime()
        .block_on(async { server.client.search_receivable_all().await });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"RPC control is disabled\"".to_string())
    );
}
