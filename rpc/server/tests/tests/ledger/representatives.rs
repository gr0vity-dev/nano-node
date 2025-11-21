use indexmap::IndexMap;
use rsnano_ledger::DEV_GENESIS_ACCOUNT;
use rsnano_types::Amount;
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn representatives_rpc_response() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let result = node
        .runtime()
        .block_on(async { server.client.representatives().await.unwrap() });

    let mut representatives = IndexMap::new();
    representatives.insert(*DEV_GENESIS_ACCOUNT, Amount::MAX);

    assert_eq!(result.representatives, representatives);
}
