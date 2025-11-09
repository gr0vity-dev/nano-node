use rsnano_types::WalletId;
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn wallet_locked_false() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet_id: WalletId = 1.into();

    node.wallet_services().wallets.create(wallet_id);

    assert_eq!(
        node.wallet_services().wallets.valid_password(&wallet_id).unwrap(),
        true
    );

    let result = node
        .runtime
        .block_on(async { server.client.wallet_locked(wallet_id).await.unwrap() });

    assert_eq!(result.locked, false.into());
}

#[test]
fn wallet_locked_true() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let wallet_id: WalletId = 1.into();

    node.wallet_services().wallets.create(wallet_id);

    node.wallet_services().wallets.lock(&wallet_id).unwrap();

    let result = node
        .runtime
        .block_on(async { server.client.wallet_locked(wallet_id).await.unwrap() });

    assert_eq!(result.locked, true.into());
}

#[test]
fn wallet_locked_fails_with_wallet_not_found() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let result = node
        .runtime
        .block_on(async { server.client.wallet_locked(WalletId::random()).await });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"Wallet not found\"".to_string())
    );
}
