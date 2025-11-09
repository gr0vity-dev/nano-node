use rsnano_types::{Account, WalletId};
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn wallet_contains_true() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet: WalletId = 1.into();

    node.wallet_services().wallets.create(1.into());

    let account = node
        .wallet_services()
        .wallets
        .deterministic_insert2(&wallet, false)
        .unwrap()
        .into();

    assert!(node.wallet_services().wallets.exists(&account));

    let result = node.runtime.block_on(async {
        server
            .client
            .wallet_contains(wallet, account.into())
            .await
            .unwrap()
    });

    assert_eq!(result.exists, true.into());
}

#[test]
fn wallet_contains_false() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet: WalletId = 1.into();

    node.wallet_services().wallets.create(1.into());

    let result = node.runtime.block_on(async {
        server
            .client
            .wallet_contains(wallet, Account::ZERO)
            .await
            .unwrap()
    });

    assert_eq!(result.exists, false.into());
}

#[test]
fn wallet_contains_fails_with_wallet_not_found() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let result = node.runtime.block_on(async {
        server
            .client
            .wallet_contains(WalletId::random(), Account::ZERO)
            .await
    });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"Wallet not found\"".to_string())
    );
}
