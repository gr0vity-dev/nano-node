use rsnano_rpc_messages::WalletAddArgs;
use rsnano_types::{PublicKey, RawKey, WalletId};
use test_helpers::{System, assert_timely2, setup_rpc_client_and_server};

#[test]
fn account_create_index_none() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet_id = WalletId::random();

    node.wallet_services().wallets.create(wallet_id);

    let private_key = RawKey::random();
    let public_key: PublicKey = private_key.into();

    node.runtime.block_on(async {
        server
            .client
            .wallet_add(WalletAddArgs::new(wallet_id, private_key))
            .await
            .unwrap()
    });

    assert!(
        node.wallet_services()
            .wallets
            .get_accounts_of_wallet(&wallet_id)
            .unwrap()
            .contains(&public_key.into())
    );
}

#[test]
fn account_create_fails_without_enable_control() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let wallet_id = WalletId::random();

    node.wallet_services().wallets.create(wallet_id);

    let private_key = RawKey::random();

    let result = node.runtime.block_on(async {
        server
            .client
            .wallet_add(WalletAddArgs::new(wallet_id, private_key))
            .await
    });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"RPC control is disabled\"".to_string())
    );
}

#[test]
fn wallet_add_fails_with_wallet_not_found() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let result = node.runtime.block_on(async {
        server
            .client
            .wallet_add(WalletAddArgs::new(WalletId::random(), RawKey::ZERO))
            .await
    });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"Wallet not found\"".to_string())
    );
}

#[test]
fn wallet_add_work_true() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet_id = WalletId::random();

    node.wallet_services().wallets.create(wallet_id);

    let private_key = RawKey::random();

    let result = node.runtime.block_on(async {
        server
            .client
            .wallet_add(WalletAddArgs::new(wallet_id, private_key))
            .await
            .unwrap()
    });

    assert_timely2(|| {
        !node
            .wallet_services()
            .wallets
            .work_get2(&wallet_id, &result.account.into())
            .unwrap()
            .is_zero()
    });
}

#[test]
fn wallet_add_work_false() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet_id = WalletId::random();

    node.wallet_services().wallets.create(wallet_id);

    let private_key = RawKey::random();

    let args = WalletAddArgs::builder(wallet_id, private_key)
        .without_precomputed_work()
        .build();

    let result = node
        .runtime
        .block_on(async { server.client.wallet_add(args).await.unwrap() });

    assert_timely2(|| {
        node.wallet_services()
            .wallets
            .work_get2(&wallet_id, &result.account.into())
            .unwrap()
            .is_zero()
    });
}
