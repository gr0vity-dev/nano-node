use rsnano_ledger::test_helpers::UnsavedBlockLatticeBuilder;
use rsnano_node::Node;
use rsnano_rpc_messages::LedgerArgs;
use rsnano_types::{Amount, Block, DEV_GENESIS_KEY, PrivateKey, StateBlockArgs};
use std::sync::Arc;
use test_helpers::{System, setup_rpc_client_and_server};

fn setup_test_environment(node: Arc<Node>) -> (PrivateKey, Block, Block) {
    let keys = PrivateKey::new();
    let rep_weight = Amount::MAX - Amount::raw(100);

    let mut lattice = UnsavedBlockLatticeBuilder::new();

    let send = lattice.genesis().send(&keys, rep_weight);
    node.process(send.clone());

    let open = lattice.account(&keys).receive(&send);
    node.process(open.clone());

    (keys, send, open)
}

#[test]
fn test_ledger() {
    let mut system = System::new();
    let node = system.build_node().finish();
    let server = setup_rpc_client_and_server(node.clone(), true);

    let (keys, _, open) = setup_test_environment(node.clone());

    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let args = LedgerArgs::builder().count(1).sorted().build();

    let result = node
        .runtime()
        .block_on(async { server.client.ledger(args).await.unwrap() });

    let accounts = result.accounts;
    assert_eq!(accounts.len(), 1);

    for (account, info) in accounts {
        assert_eq!(keys.account(), account);
        assert_eq!(open.hash(), info.frontier);
        assert_eq!(open.hash(), info.open_block);
        assert_eq!(open.hash(), info.representative_block);
        assert_eq!(Amount::MAX - Amount::raw(100), info.balance);
        assert!(((time as i64) - (info.modified_timestamp.inner() as i64)).abs() < 5);
        assert_eq!(info.block_count, 1.into());
        assert!(info.weight.is_none());
        assert!(info.pending.is_none());
        assert!(info.representative.is_none());
    }
}

#[test]
fn test_ledger_threshold() {
    let mut system = System::new();
    let node = system.build_node().finish();
    let server = setup_rpc_client_and_server(node.clone(), true);

    let (keys, _, _) = setup_test_environment(node.clone());

    let args = LedgerArgs::builder()
        .count(2)
        .sorted()
        .with_minimum_balance(Amount::MAX - Amount::raw(100))
        .build();

    let result = node
        .runtime()
        .block_on(async { server.client.ledger(args).await.unwrap() });

    let accounts = result.accounts;
    assert_eq!(accounts.len(), 1);
    assert!(accounts.contains_key(&keys.account()));
}

#[test]
fn test_ledger_pending() {
    let mut system = System::new();
    let node = system.build_node().finish();
    let server = setup_rpc_client_and_server(node.clone(), true);

    let (keys, send_block, _) = setup_test_environment(node.clone());

    let send_amount = Amount::MAX - Amount::raw(100);
    let send2_amount = Amount::raw(50);
    let new_remaining_balance = Amount::MAX - send_amount - send2_amount;

    let send2_block: Block = StateBlockArgs {
        key: &DEV_GENESIS_KEY,
        previous: send_block.hash(),
        representative: keys.public_key(),
        balance: new_remaining_balance,
        link: keys.account().into(),
        work: node.work_generate_dev(send_block.hash()),
    }
    .into();

    node.process(send2_block);

    let args = LedgerArgs::builder()
        .count(2)
        .include_receivables()
        .with_minimum_balance(Amount::MAX - Amount::raw(100))
        .build();

    let result = node
        .runtime()
        .block_on(async { server.client.ledger(args).await.unwrap() });

    let accounts = result.accounts;
    assert_eq!(accounts.len(), 1);
    let account_info = accounts.get(&keys.account()).unwrap();
    assert_eq!(account_info.balance, send_amount);
    assert_eq!(account_info.pending, Some(send2_amount));
}
