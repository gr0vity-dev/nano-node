use rsnano_ledger::{
    AnySet, DEV_GENESIS_ACCOUNT, DEV_GENESIS_HASH, DEV_GENESIS_PUB_KEY, LedgerSet,
};
use rsnano_node::Node;
use rsnano_rpc_messages::{AccountsReceivableArgs, AccountsReceivableResponse};
use rsnano_types::{
    Account, Amount, Block, DEV_GENESIS_KEY, PublicKey, RawKey, StateBlockArgs, WalletId,
};
use std::sync::Arc;
use test_helpers::{System, assert_timely2, setup_rpc_client_and_server};

fn send_block(node: Arc<Node>, account: Account, amount: Amount) -> Block {
    let ledger = node.ledger_query_services().ledger_arc();
    let any = ledger.any();

    let previous = any
        .account_head(&*DEV_GENESIS_ACCOUNT)
        .unwrap_or(*DEV_GENESIS_HASH);

    let balance = any.account_balance(&*DEV_GENESIS_ACCOUNT);

    let send: Block = StateBlockArgs {
        key: &DEV_GENESIS_KEY,
        representative: *DEV_GENESIS_PUB_KEY,
        previous,
        balance: balance - amount,
        link: account.into(),
        work: node.work_generate_dev(previous),
    }
    .into();

    node.process_active(send.clone());
    assert_timely2(|| node.is_active_root(&send.qualified_root()));

    send
}

#[test]
fn accounts_receivable_include_only_confirmed() {
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

    let _send = send_block(node.clone(), public_key.into(), Amount::raw(1));

    let server = setup_rpc_client_and_server(node.clone(), false);

    let args = AccountsReceivableArgs::build(vec![public_key.into()])
        .count(1)
        .only_confirmed(true)
        .finish();

    let result1 = node
        .runtime
        .block_on(async { server.client.accounts_receivable(args).await.unwrap() });

    if let AccountsReceivableResponse::Simple(simple) = result1 {
        assert!(simple.blocks.is_empty());
    } else {
        panic!("Expected ReceivableDto::Blocks variant");
    }

    let args = AccountsReceivableArgs::build(vec![public_key.into()])
        .count(1)
        .finish();

    let result2 = node
        .runtime
        .block_on(async { server.client.accounts_receivable(args).await })
        .unwrap();

    if let AccountsReceivableResponse::Simple(simple) = result2 {
        assert!(simple.blocks.is_empty());
    } else {
        panic!("Expected ReceivableDto::Blocks variant");
    }
}

#[test]
fn accounts_receivable_options_none() {
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

    let send = send_block(node.clone(), public_key.into(), Amount::raw(1));
    node.ledger_query_services().ledger_arc().confirm(send.hash());

    let server = setup_rpc_client_and_server(node.clone(), false);

    let args = AccountsReceivableArgs::build(vec![public_key.into()])
        .count(1)
        .only_confirmed(true)
        .finish();

    let result = node
        .runtime
        .block_on(async { server.client.accounts_receivable(args).await.unwrap() });

    if let AccountsReceivableResponse::Simple(simple) = result {
        assert_eq!(
            simple.blocks.get(&Account::from(public_key)).unwrap(),
            &vec![send.hash()]
        );
    } else {
        panic!("Expected ReceivableDto::Blocks variant");
    }
}

#[test]
fn accounts_receivable_threshold_some() {
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

    let send = send_block(node.clone(), public_key.into(), Amount::raw(1));
    node.ledger_query_services().ledger_arc().confirm(send.hash());
    let send2 = send_block(node.clone(), public_key.into(), Amount::raw(2));
    node.ledger_query_services().ledger_arc().confirm(send2.hash());

    let server = setup_rpc_client_and_server(node.clone(), false);

    let args = AccountsReceivableArgs::build(vec![public_key.into()])
        .count(1)
        .threshold(Amount::raw(1))
        .finish();

    let result = node
        .runtime
        .block_on(async { server.client.accounts_receivable(args).await.unwrap() });

    if let AccountsReceivableResponse::Threshold(threshold) = result {
        assert_eq!(
            threshold
                .blocks
                .get(&Account::from(public_key))
                .unwrap()
                .get(&send2.hash())
                .unwrap(),
            &Amount::raw(2)
        );
    } else {
        panic!("Expected ReceivableDto::Threshold variant");
    }
}

#[test]
fn accounts_receivable_sorted() {
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

    let send = send_block(node.clone(), public_key.into(), Amount::raw(1));

    let server = setup_rpc_client_and_server(node.clone(), false);

    let args = AccountsReceivableArgs::build(vec![public_key.into()])
        .count(1)
        .threshold(Amount::raw(1))
        .only_confirmed(false)
        .sorted()
        .finish();

    let result = node
        .runtime
        .block_on(async { server.client.accounts_receivable(args).await.unwrap() });

    if let AccountsReceivableResponse::Threshold(threshold) = result {
        assert_eq!(threshold.blocks.len(), 1);
        let (recv_account, recv_blocks) = threshold.blocks.iter().next().unwrap();
        assert_eq!(recv_account, &public_key.into());
        assert_eq!(recv_blocks.len(), 1);
        let (block_hash, amount) = recv_blocks.iter().next().unwrap();
        assert_eq!(block_hash, &send.hash());
        assert_eq!(amount, &Amount::raw(1));
    } else {
        panic!("Expected ReceivableDto::Threshold variant");
    }
}
