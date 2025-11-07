use rsnano_ledger::{AnySet, DEV_GENESIS_ACCOUNT, LedgerSet};
use rsnano_rpc_messages::SendArgs;
use rsnano_types::{Account, Amount, DEV_GENESIS_KEY, WalletId};
use std::time::Duration;
use test_helpers::{System, assert_timely_msg, setup_rpc_client_and_server};

#[test]
fn send() {
    let mut system = System::new();
    let node = system.make_node();

    let wallet = WalletId::random();
    node.services().wallets.create(wallet);
    node.services()
        .wallets
        .insert_adhoc2(&wallet, &DEV_GENESIS_KEY.raw_key(), false)
        .unwrap();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let destination =
        Account::parse("nano_3t6k35gi95xu6tergt6p69ck76ogmitsa8mnijtpxm9fkcm736xtoncuohr3")
            .unwrap();
    let amount = Amount::raw(1000000);

    let result = node.runtime.block_on(async {
        server
            .client
            .send(SendArgs {
                wallet,
                source: *DEV_GENESIS_ACCOUNT,
                destination,
                amount,
                ..Default::default()
            })
            .await
            .unwrap()
    });

    let any = node.services().ledger.any();

    assert_timely_msg(
        Duration::from_secs(5),
        || any.get_block(&result.block).is_some(),
        "Send block not found in ledger",
    );

    assert_eq!(
        any.account_balance(&DEV_GENESIS_ACCOUNT),
        Amount::MAX - amount
    );
}

#[test]
fn send_fails_without_enable_control() {
    let mut system = System::new();
    let node = system.make_node();

    let wallet = WalletId::random();
    node.services().wallets.create(wallet);
    node.services()
        .wallets
        .insert_adhoc2(&wallet, &DEV_GENESIS_KEY.raw_key(), false)
        .unwrap();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let destination =
        Account::parse("nano_3t6k35gi95xu6tergt6p69ck76ogmitsa8mnijtpxm9fkcm736xtoncuohr3")
            .unwrap();
    let amount = Amount::raw(1000000);

    let result = node.runtime.block_on(async {
        server
            .client
            .send(SendArgs {
                wallet,
                source: *DEV_GENESIS_ACCOUNT,
                destination,
                amount,
                ..Default::default()
            })
            .await
    });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"RPC control is disabled\"".to_string())
    );
}
