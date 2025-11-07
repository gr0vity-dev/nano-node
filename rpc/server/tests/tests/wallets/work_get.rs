use rsnano_types::{Account, WalletId, WorkNonce};
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn work_get() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet = WalletId::random();
    let account = Account::ZERO;

    node.services.wallets.create(wallet);

    node.services
        .wallets
        .work_set(&wallet, &account.into(), 1.into())
        .unwrap();

    let result = node
        .runtime
        .block_on(async { server.client.work_get(wallet, account).await.unwrap() });

    assert_eq!(result.work, WorkNonce::from(1));
}

#[test]
fn work_get_fails_without_enable_control() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let result = node.runtime.block_on(async {
        server
            .client
            .work_get(WalletId::random(), Account::ZERO)
            .await
    });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"RPC control is disabled\"".to_string())
    );
}

#[test]
fn work_get_fails_with_wallet_not_found() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let result = node.runtime.block_on(async {
        server
            .client
            .work_get(WalletId::random(), Account::ZERO)
            .await
    });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"Wallet not found\"".to_string())
    );
}
