use rsnano_ledger::test_helpers::UnsavedBlockLatticeBuilder;
use rsnano_node::Node;
use rsnano_rpc_messages::UnopenedArgs;
use rsnano_types::{Account, Amount};
use std::sync::Arc;
use test_helpers::{System, assert_timely2, setup_rpc_client_and_server};

fn send_block(node: Arc<Node>) {
    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let send = lattice.genesis().send(Account::ZERO, 1);

    node.process_active(send.clone());
    assert_timely2(|| node.is_active_root(&send.qualified_root()));
}

#[test]
fn unopened() {
    let mut system = System::new();
    let node = system.make_node();

    send_block(node.clone());

    let server = setup_rpc_client_and_server(node.clone(), true);

    let result = node.runtime().block_on(async {
        server
            .client
            .unopened(UnopenedArgs {
                account: Some(Account::ZERO),
                ..Default::default()
            })
            .await
            .unwrap()
    });

    assert_eq!(
        result.accounts.get(&Account::ZERO).unwrap(),
        &Amount::raw(1)
    );
}

#[test]
fn unopened_with_threshold() {
    let mut system = System::new();
    let node = system.make_node();

    send_block(node.clone());

    let server = setup_rpc_client_and_server(node.clone(), true);

    let args = UnopenedArgs {
        account: Some(Account::ZERO),
        threshold: Some(Amount::nano(1)),
        ..Default::default()
    };

    let result = node
        .runtime()
        .block_on(async { server.client.unopened(args).await.unwrap() });

    assert!(result.accounts.is_empty());
}

#[test]
fn unopened_fails_without_enable_control() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let args = UnopenedArgs {
        account: Some(Account::ZERO),
        ..Default::default()
    };

    let result = node
        .runtime()
        .block_on(async { server.client.unopened(args).await });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"RPC control is disabled\"".to_string())
    );
}
