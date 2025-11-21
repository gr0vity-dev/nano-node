use rsnano_rpc_messages::AccountBalanceArgs;
use rsnano_types::{Amount, DEV_GENESIS_KEY};
use test_helpers::{System, send_block, setup_rpc_client_and_server};

#[test]
fn account_balance_default_include_only_confirmed_blocks() {
    let mut system = System::new();
    let node = system.make_node();

    send_block(node.clone());

    let server = setup_rpc_client_and_server(node.clone(), false);

    let result = node.runtime().block_on(async {
        server
            .client
            .account_balance(DEV_GENESIS_KEY.public_key().as_account())
            .await
            .unwrap()
    });

    assert_eq!(
        result.balance,
        Amount::raw(340282366920938463463374607431768211455)
    );

    assert_eq!(result.pending, Amount::ZERO);
    assert_eq!(result.receivable, Amount::ZERO);
}

#[test]
fn account_balance_include_unconfirmed_blocks() {
    let mut system = System::new();
    let node = system.make_node();

    send_block(node.clone());

    let server = setup_rpc_client_and_server(node.clone(), false);

    let args = AccountBalanceArgs::build(DEV_GENESIS_KEY.public_key().as_account())
        .include_unconfirmed_blocks()
        .finish();

    let result = node
        .runtime()
        .block_on(async { server.client.account_balance(args).await.unwrap() });

    assert_eq!(
        result.balance,
        Amount::raw(340282366920938463463374607431768211454)
    );

    assert_eq!(result.pending, Amount::raw(1));
    assert_eq!(result.receivable, Amount::raw(1));
}
