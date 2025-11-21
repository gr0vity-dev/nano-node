use rsnano_types::WalletId;
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn wallet_destroy() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet_id: WalletId = 1.into();

    node.wallet_services().wallets.create(wallet_id);

    assert!(node.wallet_services().wallets.wallet_exists(&wallet_id));

    node.runtime()
        .block_on(async { server.client.wallet_destroy(wallet_id).await.unwrap() });

    assert_eq!(
        node.wallet_services().wallets.wallet_exists(&wallet_id),
        false
    );
}

#[test]
fn wallet_destroy_fails_without_enable_control() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let wallet_id: WalletId = 1.into();

    node.wallet_services().wallets.create(wallet_id);

    let result = node
        .runtime()
        .block_on(async { server.client.wallet_destroy(wallet_id).await });

    assert!(result.is_err());
}
