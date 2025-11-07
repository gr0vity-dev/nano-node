use rsnano_ledger::DEV_GENESIS_ACCOUNT;
use rsnano_types::{Account, WalletId};
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn wallet_add_watch() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet_id = WalletId::random();

    node.services().wallets.create(wallet_id);

    node.runtime.block_on(async {
        server
            .client
            .wallet_add_watch(wallet_id, vec![*DEV_GENESIS_ACCOUNT])
            .await
            .unwrap()
    });

    assert!(
        node.services()
            .wallets
            .exists(&(*DEV_GENESIS_ACCOUNT).into())
    );
}

#[test]
fn wallet_add_watch_without_enable_control() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let wallet_id = WalletId::random();

    node.services().wallets.create(wallet_id);

    let result = node.runtime.block_on(async {
        server
            .client
            .wallet_add_watch(wallet_id, vec![Account::ZERO])
            .await
    });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"RPC control is disabled\"".to_string())
    );
}
