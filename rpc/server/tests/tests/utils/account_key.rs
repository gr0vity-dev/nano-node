use rsnano_types::{Account, WalletId};
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn account_key() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet_id = WalletId::random();

    node.wallet_services().wallets.create(wallet_id);

    let result = node
        .runtime
        .block_on(async { server.client.account_key(Account::ZERO).await.unwrap() });

    assert_eq!(result.key, Account::ZERO.into());
}
