use rsnano_rpc_messages::{AccountsReceivableResponse, WalletReceivableArgs};
use rsnano_types::{Account, Amount, PublicKey, RawKey, WalletId};
use test_helpers::{System, send_block_to, setup_rpc_client_and_server};

#[test]
fn wallet_receivable_include_only_confirmed_false() {
    let mut system = System::new();
    let node = system.make_node();

    let wallet = WalletId::random();
    node.wallet_services().wallets.create(wallet);
    let private_key = RawKey::ZERO;
    let public_key: PublicKey = private_key.into();
    node.wallet_services()
        .wallets
        .insert_adhoc2(&wallet, &private_key, false)
        .unwrap();

    let send = send_block_to(node.clone(), public_key.into(), Amount::raw(1));

    let server = setup_rpc_client_and_server(node.clone(), true);

    let args = WalletReceivableArgs {
        wallet,
        count: Some(1.into()),
        include_only_confirmed: Some(false.into()),
        ..Default::default()
    };

    let result = node
        .runtime()
        .block_on(async { server.client.wallet_receivable(args).await.unwrap() });

    if let AccountsReceivableResponse::Simple(simple) = result {
        assert_eq!(
            simple.blocks.get(&Account::from(public_key)).unwrap(),
            &vec![send.hash()]
        );
    } else {
        panic!("Expected ReceivableDto::Blocks");
    }
}

#[test]
fn wallet_receivable_options_none() {
    let mut system = System::new();
    let node = system.make_node();

    let wallet = WalletId::random();
    node.wallet_services().wallets.create(wallet);
    let private_key = RawKey::ZERO;
    let public_key: PublicKey = private_key.into();
    node.wallet_services()
        .wallets
        .insert_adhoc2(&wallet, &private_key, false)
        .unwrap();

    let send = send_block_to(node.clone(), public_key.into(), Amount::raw(1));
    node.ledger_query_services()
        .ledger_arc()
        .confirm(send.hash());

    let server = setup_rpc_client_and_server(node.clone(), true);

    let result = node.runtime().block_on(async {
        server
            .client
            .wallet_receivable(WalletReceivableArgs {
                wallet,
                count: Some(1.into()),
                ..Default::default()
            })
            .await
            .unwrap()
    });

    if let AccountsReceivableResponse::Simple(simple) = result {
        assert_eq!(
            simple.blocks.get(&Account::from(public_key)).unwrap(),
            &vec![send.hash()]
        );
    } else {
        panic!("Expected ReceivableDto::Blocks");
    }
}

#[test]
fn wallet_receivable_threshold_some() {
    let mut system = System::new();
    let node = system.make_node();

    let wallet = WalletId::random();
    node.wallet_services().wallets.create(wallet);
    let private_key = RawKey::ZERO;
    let public_key: PublicKey = private_key.into();
    node.wallet_services()
        .wallets
        .insert_adhoc2(&wallet, &private_key, false)
        .unwrap();

    let send = send_block_to(node.clone(), public_key.into(), Amount::raw(1));
    node.ledger_query_services()
        .ledger_arc()
        .confirm(send.hash());
    let send2 = send_block_to(node.clone(), public_key.into(), Amount::raw(2));
    node.ledger_query_services()
        .ledger_arc()
        .confirm(send2.hash());

    let server = setup_rpc_client_and_server(node.clone(), true);

    let args = WalletReceivableArgs {
        wallet,
        count: Some(2.into()),
        threshold: Some(Amount::raw(1)),
        ..Default::default()
    };

    let result = node
        .runtime()
        .block_on(async { server.client.wallet_receivable(args).await.unwrap() });

    if let AccountsReceivableResponse::Threshold(simple) = result {
        let account_blocks = simple.blocks.get(&Account::from(public_key)).unwrap();
        assert_eq!(account_blocks.len(), 2);
        assert_eq!(account_blocks.get(&send.hash()).unwrap(), &Amount::raw(1));
        assert_eq!(account_blocks.get(&send2.hash()).unwrap(), &Amount::raw(2));
    } else {
        panic!("Expected ReceivableDto::Threshold");
    }
}

#[test]
fn wallet_receivable_fails_without_enable_control() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let result = node.runtime().block_on(async {
        server
            .client
            .wallet_receivable(WalletReceivableArgs {
                wallet: WalletId::random(),
                ..Default::default()
            })
            .await
    });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"RPC control is disabled\"".to_string())
    );
}
