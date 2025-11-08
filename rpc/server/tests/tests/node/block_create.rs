use rsnano_ledger::{AnySet, DEV_GENESIS_ACCOUNT, DEV_GENESIS_HASH};
use rsnano_rpc_messages::{BlockCreateArgs, BlockTypeDto};
use rsnano_types::{Amount, Block, BlockType, DEV_GENESIS_KEY, PrivateKey, WalletId};
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn block_create_state() {
    let mut system = System::new();
    let mut config = System::default_config();
    config.online_weight_minimum = Amount::MAX;
    let node = system.build_node().config(config).finish();

    let wallet_id = WalletId::random();
    node.wallet_services().wallets.create(wallet_id);
    node.wallet_services()
        .wallets
        .insert_adhoc2(&wallet_id, &DEV_GENESIS_KEY.raw_key(), false)
        .unwrap();
    let key1 = PrivateKey::new();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let result = node.runtime.block_on(async {
        server
            .client
            .block_create(BlockCreateArgs::new(
                BlockTypeDto::State,
                Some(Amount::MAX - Amount::raw(100)),
                Some(DEV_GENESIS_KEY.raw_key()),
                None,
                Some(*DEV_GENESIS_ACCOUNT),
                None,
                Some(key1.account()),
                Some(key1.account()),
                Some((*DEV_GENESIS_ACCOUNT).into()),
                Some(*DEV_GENESIS_HASH),
                None,
                None,
                None,
            ))
            .await
            .unwrap()
    });

    let block_hash = result.hash;
    let block: Block = result.block.into();

    assert_eq!(block.block_type(), BlockType::State);
    assert_eq!(block.hash(), block_hash);

    node.process(block.clone());

    assert_eq!(
        node.ledger_query_services()
            .ledger
            .any()
            .block_account(&block.hash()),
        Some(*DEV_GENESIS_ACCOUNT)
    );
}
