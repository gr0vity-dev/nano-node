use rsnano_ledger::{AnySet, DEV_GENESIS_ACCOUNT, DEV_GENESIS_HASH, LedgerSet};
use rsnano_rpc_messages::ReceiveArgs;
use rsnano_types::{Amount, BlockHash, DEV_GENESIS_KEY, WalletId};
use test_helpers::{System, assert_timely2, setup_rpc_client_and_server};

#[test]
fn receive() {
    let mut system = System::new();
    let node = system.make_node();

    let wallet = WalletId::random();
    node.wallet_services().wallets.create(wallet);
    node.wallet_services()
        .wallets
        .insert_adhoc2(&wallet, &DEV_GENESIS_KEY.raw_key(), false)
        .unwrap();

    let key1 = rsnano_types::PrivateKey::new();
    node.wallet_services()
        .wallets
        .insert_adhoc2(&wallet, &key1.raw_key(), false)
        .unwrap();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let send1 = node
        .wallet_services()
        .wallets
        .send(
            wallet,
            *DEV_GENESIS_ACCOUNT,
            key1.public_key().into(),
            node.config.receive_minimum,
            node.work_generate_dev(*DEV_GENESIS_HASH),
            true,
            None,
        )
        .wait()
        .unwrap();

    assert_timely2(|| {
        node.ledger_query_services()
            .ledger_arc()
            .any()
            .account_balance(&*DEV_GENESIS_ACCOUNT)
            != Amount::MAX
    });

    assert_timely2(|| {
        !node
            .ledger_query_services()
            .ledger_arc()
            .any()
            .get_account(&key1.public_key().into())
            .is_some()
    });

    let send2 = node
        .wallet_services()
        .wallets
        .send(
            wallet,
            *DEV_GENESIS_ACCOUNT,
            key1.public_key().into(),
            node.config.receive_minimum - Amount::raw(1),
            node.work_generate_dev(send1.hash()),
            true,
            None,
        )
        .wait()
        .unwrap();

    let args = ReceiveArgs::builder(wallet, key1.public_key().into(), send2.hash()).build();

    let block_hash = node
        .runtime
        .block_on(async { server.client.receive(args).await.unwrap() })
        .block;

    let ledger_query_services = node.ledger_query_services();
    let ledger = ledger_query_services.ledger_arc();
    let any = ledger.any();
    assert_timely2(|| any.get_block(&block_hash).is_some());

    assert_eq!(
        any.account_balance(&key1.public_key().into()),
        node.config.receive_minimum - Amount::raw(1)
    );

    let args = ReceiveArgs::builder(wallet, key1.public_key().into(), send2.hash()).build();

    let error_result = node
        .runtime
        .block_on(async { server.client.receive(args).await });

    assert_eq!(
        error_result.err().map(|e| e.to_string()),
        Some("node returned error: \"Block is not receivable\"".to_string())
    );

    let args = ReceiveArgs::builder(wallet, key1.public_key().into(), BlockHash::ZERO).build();

    let error_result = node
        .runtime
        .block_on(async { server.client.receive(args).await });

    assert_eq!(
        error_result.err().map(|e| e.to_string()),
        Some("node returned error: \"Block not found\"".to_string())
    );
}
