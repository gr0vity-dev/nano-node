use rsnano_types::{PublicKey, WalletId};
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn account_get() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet_id = WalletId::random();

    node.services.wallets.create(wallet_id);

    let result = node
        .runtime
        .block_on(async { server.client.account_get(PublicKey::ZERO).await.unwrap() });

    assert_eq!(result.account, PublicKey::ZERO.into());
}
