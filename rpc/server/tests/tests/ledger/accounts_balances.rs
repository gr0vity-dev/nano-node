use rsnano_ledger::{DEV_GENESIS_ACCOUNT, test_helpers::UnsavedBlockLatticeBuilder};
use rsnano_node::Node;
use rsnano_rpc_messages::AccountsBalancesArgs;
use rsnano_types::{Amount, DEV_GENESIS_KEY};
use std::sync::Arc;
use test_helpers::{System, assert_timely2, setup_rpc_client_and_server};

fn send_block(node: Arc<Node>) {
    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let send = lattice.genesis().send(&*DEV_GENESIS_KEY, 1);

    node.process_active(send.clone());
    assert_timely2(|| node.is_active_root(&send.qualified_root()));
}

#[test]
fn accounts_balances_only_confirmed_none() {
    let mut system = System::new();
    let node = system.make_node();

    send_block(node.clone());

    let server = setup_rpc_client_and_server(node.clone(), false);

    let result = node.runtime().block_on(async {
        server
            .client
            .accounts_balances(vec![DEV_GENESIS_KEY.public_key().as_account()])
            .await
            .unwrap()
    });

    let account = result.balances.get(&DEV_GENESIS_ACCOUNT).unwrap();

    assert_eq!(
        account.balance,
        Amount::raw(340282366920938463463374607431768211455)
    );
    assert_eq!(account.pending, Amount::ZERO);
    assert_eq!(account.receivable, Amount::ZERO);
}

#[test]
fn account_balance_only_confirmed_true() {
    let mut system = System::new();
    let node = system.make_node();

    send_block(node.clone());

    let server = setup_rpc_client_and_server(node.clone(), false);

    let result = node.runtime().block_on(async {
        server
            .client
            .accounts_balances(vec![DEV_GENESIS_KEY.public_key().as_account()])
            .await
            .unwrap()
    });

    let account = result.balances.get(&DEV_GENESIS_ACCOUNT).unwrap();

    assert_eq!(
        account.balance,
        Amount::raw(340282366920938463463374607431768211455)
    );

    assert_eq!(account.pending, Amount::ZERO);
    assert_eq!(account.receivable, Amount::ZERO);
}

#[test]
fn account_balance_only_confirmed_false() {
    let mut system = System::new();
    let node = system.make_node();

    send_block(node.clone());

    let server = setup_rpc_client_and_server(node.clone(), false);

    let args = AccountsBalancesArgs::new(vec![DEV_GENESIS_KEY.public_key().as_account()])
        .include_unconfirmed_blocks()
        .finish();

    let result = node
        .runtime()
        .block_on(async { server.client.accounts_balances(args).await.unwrap() });

    let account = result.balances.get(&DEV_GENESIS_ACCOUNT).unwrap();

    assert_eq!(
        account.balance,
        Amount::raw(340282366920938463463374607431768211454)
    );

    assert_eq!(account.pending, Amount::raw(1));
    assert_eq!(account.receivable, Amount::raw(1));
}
