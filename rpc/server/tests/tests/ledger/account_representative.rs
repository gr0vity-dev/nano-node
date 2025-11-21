use rsnano_ledger::DEV_GENESIS_ACCOUNT;
use rsnano_types::Account;
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn account_representative() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let result = node.runtime().block_on(async {
        server
            .client
            .account_representative(*DEV_GENESIS_ACCOUNT)
            .await
            .unwrap()
    });

    assert_eq!(result.representative, *DEV_GENESIS_ACCOUNT);
}

#[test]
fn account_representative_fails_with_account_not_found() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let result = node
        .runtime()
        .block_on(async { server.client.account_representative(Account::ZERO).await });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"Account not found\"".to_string())
    );
}
