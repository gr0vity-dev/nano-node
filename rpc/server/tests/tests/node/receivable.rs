use rsnano_ledger::{
    AnySet, DEV_GENESIS_ACCOUNT, DEV_GENESIS_HASH, DEV_GENESIS_PUB_KEY, LedgerSet,
};
use rsnano_node::Node;
use rsnano_rpc_messages::{ReceivableArgs, ReceivableResponse};
use rsnano_types::{
    Account, Amount, Block, DEV_GENESIS_KEY, PublicKey, RawKey, StateBlockArgs, WalletId,
};
use std::sync::Arc;
use test_helpers::{System, assert_timely2, setup_rpc_client_and_server};

fn send_block(node: Arc<Node>, account: Account, amount: Amount) -> Block {
    let any = node.services.ledger.any();

    let previous = any
        .account_head(&*DEV_GENESIS_ACCOUNT)
        .unwrap_or(*DEV_GENESIS_HASH);

    let balance = any.account_balance(&*DEV_GENESIS_ACCOUNT);

    let send: Block = StateBlockArgs {
        key: &DEV_GENESIS_KEY,
        previous,
        representative: *DEV_GENESIS_PUB_KEY,
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
fn receivable_include_only_confirmed() {
    let mut system = System::new();
    let node = system.make_node();

    let wallet = WalletId::random();
    node.services.wallets.create(wallet);
    let private_key = RawKey::ZERO;
    let public_key: PublicKey = private_key.into();
    node.services
        .wallets
        .insert_adhoc2(&wallet, &private_key, false)
        .unwrap();

    let send = send_block(node.clone(), public_key.into(), Amount::raw(1));

    let server = setup_rpc_client_and_server(node.clone(), false);

    let result1 = node.runtime.block_on(async {
        server
            .client
            .receivable(ReceivableArgs {
                account: public_key.into(),
                count: Some(1.into()),
                ..Default::default()
            })
            .await
            .unwrap()
    });

    if let ReceivableResponse::Simple(simple) = result1 {
        assert!(simple.blocks.is_empty());
    } else {
        panic!("Expected ReceivableDto::Blocks variant");
    }

    let args = ReceivableArgs::build(public_key)
        .count(1)
        .include_only_confirmed(false)
        .finish();

    let result2 = node
        .runtime
        .block_on(async { server.client.receivable(args).await.unwrap() });

    if let ReceivableResponse::Simple(simple) = result2 {
        assert_eq!(simple.blocks, vec![send.hash()]);
    } else {
        panic!("Expected ReceivableDto::Blocks variant");
    }
}

#[test]
fn receivable_options_none() {
    let mut system = System::new();
    let node = system.make_node();

    let wallet = WalletId::random();
    node.services.wallets.create(wallet);
    let private_key = RawKey::ZERO;
    let public_key: PublicKey = private_key.into();
    node.services
        .wallets
        .insert_adhoc2(&wallet, &private_key, false)
        .unwrap();

    let send = send_block(node.clone(), public_key.into(), Amount::raw(1));
    node.services.ledger.confirm(send.hash());

    let server = setup_rpc_client_and_server(node.clone(), false);

    let result = node.runtime.block_on(async {
        server
            .client
            .receivable(Account::from(public_key))
            .await
            .unwrap()
    });

    if let ReceivableResponse::Simple(simple) = result {
        assert_eq!(simple.blocks, vec![send.hash()]);
    } else {
        panic!("Expected ReceivableDto::Blocks variant");
    }
}

#[test]
fn receivable_threshold_some() {
    let mut system = System::new();
    let node = system.make_node();

    let wallet = WalletId::random();
    node.services.wallets.create(wallet);
    let private_key = RawKey::ZERO;
    let public_key: PublicKey = private_key.into();
    node.services
        .wallets
        .insert_adhoc2(&wallet, &private_key, false)
        .unwrap();

    let send = send_block(node.clone(), public_key.into(), Amount::raw(1));
    node.services.ledger.confirm(send.hash());
    let send2 = send_block(node.clone(), public_key.into(), Amount::raw(2));
    node.services.ledger.confirm(send2.hash());

    let server = setup_rpc_client_and_server(node.clone(), false);

    let args = ReceivableArgs::build(public_key)
        .count(2)
        .threshold(Amount::raw(1))
        .finish();

    let result = node
        .runtime
        .block_on(async { server.client.receivable(args).await.unwrap() });

    if let ReceivableResponse::Threshold(threshold) = result {
        assert_eq!(
            threshold.blocks.get(&send2.hash()).unwrap(),
            &Amount::raw(2)
        );
    } else {
        panic!("Expected ReceivableDto::Threshold variant");
    }
}
