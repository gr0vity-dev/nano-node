use rsnano_ledger::DEV_GENESIS_ACCOUNT;
use rsnano_rpc_messages::ChainArgs;
use rsnano_types::{Amount, DEV_GENESIS_KEY, PrivateKey, WalletId};
use test_helpers::{System, assert_timely2, setup_rpc_client_and_server};

#[test]
fn successors() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let wallet_id = WalletId::random();
    node.wallet_services().wallets.create(wallet_id);
    node.wallet_services()
        .wallets
        .insert_adhoc2(&wallet_id, &DEV_GENESIS_KEY.raw_key(), true)
        .unwrap();

    let genesis = node.latest(&*DEV_GENESIS_ACCOUNT);
    assert!(!genesis.is_zero());

    let key = PrivateKey::new();
    let block = node
        .wallet_services()
        .wallets
        .send(
            wallet_id,
            *DEV_GENESIS_ACCOUNT,
            key.account(),
            Amount::raw(1),
            0.into(),
            true,
            None,
        )
        .wait()
        .unwrap();

    assert_timely2(|| node.is_active_root(&block.qualified_root()));

    let result = node.runtime.block_on(async {
        server
            .client
            .successors(ChainArgs::builder(genesis, u64::MAX).build())
            .await
            .unwrap()
    });

    let blocks = result.blocks.clone();

    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0], genesis);
    assert_eq!(blocks[1], block.hash());

    let args = ChainArgs::builder(genesis, u64::MAX).reverse().build();

    let reverse_result = node
        .runtime
        .block_on(async { server.client.chain(args).await.unwrap() });

    assert_eq!(result, reverse_result);
}
