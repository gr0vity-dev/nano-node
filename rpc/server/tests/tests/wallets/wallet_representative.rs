use rsnano_types::{PublicKey, WalletId};
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn wallet_representative() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet = WalletId::random();
    node.wallet_services().wallets.create(wallet);
    node.wallet_services()
        .wallets
        .set_representative(wallet, PublicKey::ZERO, false)
        .wait()
        .unwrap();

    let result = node
        .runtime
        .block_on(async { server.client.wallet_representative(wallet).await.unwrap() });

    assert_eq!(result.representative, PublicKey::ZERO.into());
}

#[test]
fn wallet_representative_fails_with_wallet_not_found() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let result = node.runtime.block_on(async {
        server
            .client
            .wallet_representative(WalletId::random())
            .await
    });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"Wallet not found\"".to_string())
    );
}
