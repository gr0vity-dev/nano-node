use rsnano_ledger::test_helpers::UnsavedBlockLatticeBuilder;
use rsnano_node::Node;
use rsnano_rpc_messages::ReceivableExistsArgs;
use rsnano_types::{Block, BlockHash, DEV_GENESIS_KEY};
use std::sync::Arc;
use test_helpers::{System, assert_timely2, setup_rpc_client_and_server};

fn send_block(node: Arc<Node>) -> Block {
    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let send1 = lattice.genesis().send(&*DEV_GENESIS_KEY, 1);
    node.process_active(send1.clone());
    assert_timely2(|| node.is_active_root(&send1.qualified_root()));
    send1
}

#[test]
fn receivable_exists_confirmed() {
    let mut system = System::new();
    let node = system.make_node();

    let send = send_block(node.clone());
    node.confirm(send.hash().clone());

    let server = setup_rpc_client_and_server(node.clone(), false);

    let result = node
        .runtime()
        .block_on(async { server.client.receivable_exists(send.hash()).await.unwrap() });

    assert_eq!(result.exists, true.into());
}

#[test]
fn test_receivable_exists_unconfirmed() {
    let mut system = System::new();
    let node = system.make_node();

    let send = send_block(node.clone());

    let server = setup_rpc_client_and_server(node.clone(), false);

    let args = ReceivableExistsArgs::build(send.hash())
        .include_active()
        .include_unconfirmed_blocks()
        .finish();

    let result = node
        .runtime()
        .block_on(async { server.client.receivable_exists(args).await.unwrap() });

    assert_eq!(result.exists, true.into());
}

#[test]
fn test_receivable_exists_non_existent() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let non_existent_hash = BlockHash::ZERO;
    let result = node
        .runtime()
        .block_on(async { server.client.receivable_exists(non_existent_hash).await })
        .unwrap_err();

    assert!(result.to_string().contains("Block not found"));
}
