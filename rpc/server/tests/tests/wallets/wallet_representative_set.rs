use rsnano_rpc_messages::WalletRepresentativeSetArgs;
use rsnano_types::{Account, PublicKey, WalletId};
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn wallet_representative_set() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet = WalletId::random();
    node.wallet_services().wallets.create(wallet);

    node.runtime.block_on(async {
        server
            .client
            .wallet_representative_set(WalletRepresentativeSetArgs::new(wallet, Account::ZERO))
            .await
            .unwrap()
    });

    assert_eq!(
        node.wallet_services().wallets.get_representative(wallet).unwrap(),
        PublicKey::ZERO
    );
}

#[test]
fn wallet_representative_set_fails_without_enable_control() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let result = node.runtime.block_on(async {
        server
            .client
            .wallet_representative_set(WalletRepresentativeSetArgs::new(
                WalletId::random(),
                Account::ZERO,
            ))
            .await
    });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"RPC control is disabled\"".to_string())
    );
}
