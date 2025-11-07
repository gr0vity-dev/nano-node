use rsnano_ledger::test_helpers::UnsavedBlockLatticeBuilder;
use rsnano_node::Node;
use rsnano_rpc_messages::WalletLedgerArgs;
use rsnano_types::{Amount, BlockHash, PrivateKey, WalletId};
use std::sync::Arc;
use test_helpers::{System, setup_rpc_client_and_server};

fn setup_test_environment(node: Arc<Node>, keys: PrivateKey, send_amount: Amount) -> BlockHash {
    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let send1 = lattice.genesis().send(&keys, send_amount);
    node.process(send1.clone());

    let open_block = lattice.account(&keys).receive(&send1);
    node.process(open_block.clone());

    open_block.hash()
}

#[test]
fn wallet_ledger() {
    let mut system = System::new();
    let node = system.build_node().finish();
    let keys = PrivateKey::new();
    let send_amount = Amount::from(100);
    let open_hash = setup_test_environment(node.clone(), keys.clone(), send_amount);

    let wallet_id = WalletId::random();
    node.services().wallets.create(wallet_id);
    node.services()
        .wallets
        .insert_adhoc2(&wallet_id, &keys.raw_key(), true)
        .unwrap();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let args = WalletLedgerArgs::builder(wallet_id)
        .receivable()
        .representative()
        .weight()
        .build();

    let result = node
        .runtime
        .block_on(async { server.client.wallet_ledger(args).await.unwrap() });

    let accounts = result.accounts;

    assert_eq!(accounts.len(), 1);
    let (account, info) = accounts.iter().next().unwrap();
    assert_eq!(*account, keys.account());
    assert_eq!(info.frontier, BlockHash::from(open_hash));
    assert_eq!(info.open_block, BlockHash::from(open_hash));
    assert_eq!(info.representative_block, BlockHash::from(open_hash));
    assert_eq!(info.balance, send_amount);
    assert!(info.modified_timestamp.inner() > 0);
    assert_eq!(info.block_count, 1.into());
    assert_eq!(info.weight, Some(send_amount));
    assert_eq!(info.pending, Some(Amount::ZERO));
    assert_eq!(info.receivable, Some(Amount::ZERO));
    assert_eq!(info.representative, Some(keys.account()));

    let result_without_optional = node
        .runtime
        .block_on(async { server.client.wallet_ledger(wallet_id).await.unwrap() });

    let accounts_without_optional = result_without_optional.accounts;
    let (_, info_without_optional) = accounts_without_optional.iter().next().unwrap();
    assert!(info_without_optional.weight.is_none());
    assert!(info_without_optional.pending.is_none());
    assert!(info_without_optional.receivable.is_none());
    assert!(info_without_optional.representative.is_none());
}

#[test]
fn account_create_fails_without_enable_control() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let result = node
        .runtime
        .block_on(async { server.client.wallet_ledger(WalletId::random()).await });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"RPC control is disabled\"".to_string())
    );
}

#[test]
fn account_create_fails_with_wallet_not_found() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let result = node
        .runtime
        .block_on(async { server.client.wallet_ledger(WalletId::random()).await });

    assert_eq!(
        result.err().map(|e| e.to_string()),
        Some("node returned error: \"Wallet not found\"".to_string())
    );
}
