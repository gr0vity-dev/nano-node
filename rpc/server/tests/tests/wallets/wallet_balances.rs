use rsnano_ledger::test_helpers::UnsavedBlockLatticeBuilder;
use rsnano_node::Node;
use rsnano_rpc_messages::{AccountBalanceResponse, AccountsBalancesResponse, WalletBalancesArgs};
use rsnano_types::{Account, Amount, PublicKey, RawKey, WalletId};
use std::{collections::HashMap, sync::Arc};
use test_helpers::{System, assert_timely2, setup_rpc_client_and_server};

fn send_block(node: Arc<Node>, account: Account) {
    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let send1 = lattice.genesis().send(account, 1);

    node.process_active(send1.clone());
    assert_timely2(|| node.is_active_root(&send1.qualified_root()));
}

#[test]
fn wallet_balances_threshold_none() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let wallet: WalletId = 1.into();
    node.services().wallets.create(wallet);

    let result = node
        .runtime
        .block_on(async { server.client.wallet_balances(wallet).await.unwrap() });

    let expected_balances: HashMap<Account, AccountBalanceResponse> = HashMap::new();
    let expected_result = AccountsBalancesResponse {
        balances: expected_balances,
    };

    assert_eq!(result, expected_result);
}

#[test]
fn wallet_balances_threshold_some() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let wallet: WalletId = 1.into();
    let private_key = RawKey::ZERO;
    let public_key: PublicKey = private_key.into();

    node.services().wallets.create(wallet);

    node.services()
        .wallets
        .insert_adhoc2(&wallet, &RawKey::ZERO, false)
        .unwrap();

    send_block(node.clone(), public_key.into());

    let result = node.runtime.block_on(async {
        let args = WalletBalancesArgs::build(wallet)
            .with_minimum_balance(Amount::ZERO)
            .finish();
        server.client.wallet_balances(args).await.unwrap()
    });

    let mut expected_balances: HashMap<Account, AccountBalanceResponse> = HashMap::new();
    expected_balances.insert(
        public_key.into(),
        AccountBalanceResponse {
            balance: Amount::ZERO,
            pending: Amount::raw(1),
            receivable: Amount::raw(1),
        },
    );
    let expected_result = AccountsBalancesResponse {
        balances: expected_balances,
    };

    assert_eq!(result, expected_result);
}

#[test]
fn wallet_balances_threshold_some_fails() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let wallet = 1.into();
    node.services().wallets.create(wallet);

    let public_key = node
        .services()
        .wallets
        .deterministic_insert2(&1.into(), false)
        .unwrap();

    send_block(node.clone(), public_key.into());

    let result = node.runtime.block_on(async {
        let args = WalletBalancesArgs::build(wallet)
            .with_minimum_balance(Amount::nano(1))
            .finish();

        server.client.wallet_balances(args).await.unwrap()
    });

    let expected_balances: HashMap<Account, AccountBalanceResponse> = HashMap::new();
    let expected_result = AccountsBalancesResponse {
        balances: expected_balances,
    };

    assert_eq!(result, expected_result);
}
