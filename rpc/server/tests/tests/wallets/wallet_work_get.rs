use rsnano_types::{PublicKey, RawKey, WalletId, WorkNonce};
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn wallet_work_get() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet = WalletId::random();
    let private_key = RawKey::ZERO;
    let public_key = PublicKey::from(private_key);

    node.wallet_services().wallets.create(wallet);

    node.wallet_services()
        .wallets
        .insert_adhoc2(&wallet, &private_key, false)
        .unwrap();

    node.wallet_services()
        .wallets
        .work_set(&wallet, &public_key, 1.into())
        .unwrap();

    let result = node
        .runtime()
        .block_on(async { server.client.wallet_work_get(wallet).await.unwrap() });

    assert_eq!(
        result.works.get(&public_key.into()).unwrap(),
        &WorkNonce::from(1)
    );
}

#[test]
fn wallet_work_get_fails_without_enable_control() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let result = node
        .runtime()
        .block_on(async { server.client.wallet_work_get(WalletId::random()).await });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"RPC control is disabled\"".to_string())
    );
}

#[test]
fn wallet_work_get_fails_with_wallet_not_found() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let result = node
        .runtime()
        .block_on(async { server.client.wallet_work_get(WalletId::random()).await });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"Wallet not found\"".to_string())
    );
}
