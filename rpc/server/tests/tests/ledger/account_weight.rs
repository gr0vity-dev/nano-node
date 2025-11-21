use rsnano_ledger::DEV_GENESIS_ACCOUNT;
use rsnano_types::Amount;
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn account_weight() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let result = node.runtime().block_on(async {
        server
            .client
            .account_weight(DEV_GENESIS_ACCOUNT.to_owned())
            .await
            .unwrap()
    });

    assert_eq!(result.weight, Amount::MAX);
}
