use rsnano_rpc_messages::AccountCreateArgs;
use rsnano_types::WalletId;
use std::u32;
use test_helpers::{System, assert_timely2, setup_rpc_client_and_server};

#[test]
fn account_create_default() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet_id = WalletId::random();

    node.services().wallets.create(wallet_id);

    let result = node
        .runtime
        .block_on(async { server.client.account_create(wallet_id).await.unwrap() });

    assert!(node.services().wallets.exists(&result.account.into()));
}

#[test]
fn account_create_index_max() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet_id = WalletId::random();

    node.services().wallets.create(wallet_id);

    let args = AccountCreateArgs::builder(wallet_id)
        .with_index(u32::MAX)
        .build();

    let result = node
        .runtime
        .block_on(async { server.client.account_create(args).await.unwrap() });

    assert!(node.services().wallets.exists(&result.account.into()));
}

#[test]
fn account_create_work_without_precomputed_work() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet_id = WalletId::random();

    node.services().wallets.create(wallet_id);

    let args = AccountCreateArgs::builder(wallet_id)
        .without_precomputed_work()
        .build();

    let result = node
        .runtime
        .block_on(async { server.client.account_create(args).await.unwrap() });

    assert!(node.services().wallets.exists(&result.account.into()));

    assert_timely2(|| {
        node.services()
            .wallets
            .work_get2(&wallet_id, &result.account.into())
            .unwrap()
            .is_zero()
    });
}

#[test]
fn account_create_fails_without_enable_control() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let wallet_id = WalletId::random();

    node.services().wallets.create(wallet_id);

    let result = node
        .runtime
        .block_on(async { server.client.account_create(wallet_id).await });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"RPC control is disabled\"".to_string())
    );
}

#[test]
fn account_create_fails_wallet_locked() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet_id = WalletId::random();

    node.services().wallets.create(wallet_id);

    node.services().wallets.lock(&wallet_id).unwrap();

    let result = node
        .runtime
        .block_on(async { server.client.account_create(wallet_id).await });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"Wallet is locked\"".to_string())
    );
}

#[test]
fn account_create_fails_wallet_not_found() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet_id = WalletId::random();

    let result = node
        .runtime
        .block_on(async { server.client.account_create(wallet_id).await });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"Wallet not found\"".to_string())
    );
}
