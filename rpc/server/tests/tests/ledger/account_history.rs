use rsnano_ledger::{DEV_GENESIS_ACCOUNT, DEV_GENESIS_HASH, DEV_GENESIS_PUB_KEY};
use rsnano_rpc_messages::{AccountHistoryArgs, BlockTypeDto};
use rsnano_types::{Account, Amount, DEV_GENESIS_KEY, PublicKey, WalletId};
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn account_history() {
    let mut system = System::new();
    let node = system.make_node();

    // Create and process blocks
    let wallet_id = WalletId::random();
    node.wallet_services().wallets.create(wallet_id);
    node.wallet_services()
        .wallets
        .insert_adhoc2(&wallet_id, &DEV_GENESIS_KEY.raw_key(), false)
        .unwrap();

    let change = node
        .wallet_services()
        .wallets
        .change(
            &wallet_id,
            *DEV_GENESIS_ACCOUNT,
            *DEV_GENESIS_PUB_KEY,
            node.work_generate_dev(*DEV_GENESIS_HASH),
            false,
        )
        .wait()
        .unwrap();

    let send = node
        .wallet_services()
        .wallets
        .send(
            wallet_id,
            *DEV_GENESIS_ACCOUNT,
            *DEV_GENESIS_ACCOUNT,
            node.config.receive_minimum,
            node.work_generate_dev(change.hash()),
            false,
            None,
        )
        .wait()
        .unwrap();

    let receive = node
        .wallet_services()
        .wallets
        .receive(
            wallet_id,
            send.hash(),
            *DEV_GENESIS_PUB_KEY,
            node.config.receive_minimum,
            *DEV_GENESIS_ACCOUNT,
            node.work_generate_dev(send.hash()),
            false,
        )
        .wait()
        .unwrap();

    let usend = node
        .wallet_services()
        .wallets
        .send(
            wallet_id,
            *DEV_GENESIS_ACCOUNT,
            *DEV_GENESIS_ACCOUNT,
            Amount::nano(1_000),
            node.work_generate_dev(receive.hash()),
            false,
            None,
        )
        .wait()
        .unwrap();

    let ureceive = node
        .wallet_services()
        .wallets
        .receive(
            wallet_id,
            usend.hash(),
            *DEV_GENESIS_PUB_KEY,
            Amount::nano(1_000),
            *DEV_GENESIS_ACCOUNT,
            node.work_generate_dev(usend.hash()),
            false,
        )
        .wait()
        .unwrap();

    let uchange = node
        .wallet_services()
        .wallets
        .change(
            &wallet_id,
            *DEV_GENESIS_ACCOUNT,
            PublicKey::ZERO,
            node.work_generate_dev(ureceive.hash()),
            false,
        )
        .wait()
        .unwrap();

    // Set up RPC client and server
    let server = setup_rpc_client_and_server(node.clone(), true);

    let args = AccountHistoryArgs::new(*DEV_GENESIS_ACCOUNT, 100);

    let account_history = node
        .runtime
        .block_on(async { server.client.account_history(args).await.unwrap() });

    assert_eq!(account_history.account, *DEV_GENESIS_ACCOUNT);
    assert_eq!(account_history.history.len(), 5);

    // Verify history entries
    let history = account_history.history;
    assert_eq!(history[0].block_type, Some(BlockTypeDto::Receive));
    assert_eq!(history[0].hash, ureceive.hash());
    assert_eq!(history[0].account, Some(*DEV_GENESIS_ACCOUNT));
    assert_eq!(history[0].amount, Some(Amount::nano(1_000)));
    assert_eq!(history[0].height, 6.into());
    assert_eq!(history[0].confirmed, false.into());

    assert_eq!(history[1].block_type, Some(BlockTypeDto::Send));
    assert_eq!(history[1].hash, usend.hash());
    assert_eq!(history[1].account, Some(*DEV_GENESIS_ACCOUNT));
    assert_eq!(history[1].amount, Some(Amount::nano(1_000)));
    assert_eq!(history[1].height, 5.into());
    assert_eq!(history[1].confirmed, false.into());

    assert_eq!(history[2].block_type, Some(BlockTypeDto::Receive));
    assert_eq!(history[2].hash, receive.hash());
    assert_eq!(history[2].account, Some(*DEV_GENESIS_ACCOUNT));
    assert_eq!(history[2].amount, Some(node.config.receive_minimum));
    assert_eq!(history[2].height, 4.into());
    assert_eq!(history[2].confirmed, false.into());

    assert_eq!(history[3].block_type, Some(BlockTypeDto::Send));
    assert_eq!(history[3].hash, send.hash());
    assert_eq!(history[3].account, Some(*DEV_GENESIS_ACCOUNT));
    assert_eq!(history[3].amount, Some(node.config.receive_minimum));
    assert_eq!(history[3].height, 3.into());
    assert_eq!(history[3].confirmed, false.into());

    assert_eq!(history[4].block_type, Some(BlockTypeDto::Receive));
    assert_eq!(history[4].hash, *DEV_GENESIS_HASH);
    assert_eq!(history[4].account, Some(*DEV_GENESIS_ACCOUNT));
    assert_eq!(
        history[4].amount,
        Some(node.ledger_query_services().ledger_arc().constants.genesis_amount)
    );
    assert_eq!(history[4].height, 1.into());
    assert_eq!(history[4].confirmed, true.into());

    let args = AccountHistoryArgs::build_for_account(*DEV_GENESIS_ACCOUNT, 1)
        .reverse()
        .finish();

    // Test count and reverse
    let account_history_reverse = node
        .runtime
        .block_on(async { server.client.account_history(args).await.unwrap() });

    assert_eq!(account_history_reverse.history.len(), 1);
    assert_eq!(account_history_reverse.history[0].height, 1.into());
    assert_eq!(account_history_reverse.next, Some(change.hash()));

    // Test filtering
    let account2: Account = node
        .wallet_services()
        .wallets
        .deterministic_insert2(&wallet_id, false)
        .unwrap()
        .into();
    let send2 = node
        .wallet_services()
        .wallets
        .send(
            wallet_id,
            *DEV_GENESIS_ACCOUNT,
            account2,
            node.config.receive_minimum,
            node.work_generate_dev(uchange.hash()),
            false,
            None,
        )
        .wait()
        .unwrap();

    node.wallet_services()
        .wallets
        .receive(
            wallet_id,
            send2.hash(),
            account2.into(),
            node.config.receive_minimum,
            account2.into(),
            node.work_generate_dev(send2.hash()),
            false,
        )
        .wait()
        .unwrap();

    let args = AccountHistoryArgs::build_for_account(*DEV_GENESIS_ACCOUNT, 100)
        .account_filter(vec![account2])
        .finish();

    // Test filter for send state blocks
    let account_history_filtered_send = node
        .runtime
        .block_on(async { server.client.account_history(args).await.unwrap() });

    assert_eq!(account_history_filtered_send.history.len(), 2);
    assert_eq!(
        account_history_filtered_send.history[0].block_type,
        Some(BlockTypeDto::Send)
    );
    assert_eq!(
        account_history_filtered_send.history[0].account,
        Some(account2)
    );

    let args = AccountHistoryArgs::build_for_account(account2.into(), 100)
        .account_filter(vec![*DEV_GENESIS_ACCOUNT])
        .finish();

    // Test filter for receive state blocks
    let account_history_filtered_receive = node
        .runtime
        .block_on(async { server.client.account_history(args).await.unwrap() });

    assert_eq!(account_history_filtered_receive.history.len(), 1);
    assert_eq!(
        account_history_filtered_receive.history[0].block_type,
        Some(BlockTypeDto::Receive)
    );
    assert_eq!(
        account_history_filtered_receive.history[0].account,
        Some(*DEV_GENESIS_ACCOUNT)
    );
}
