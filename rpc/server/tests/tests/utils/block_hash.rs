use rsnano_types::Block;
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn block_hash() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), false);

    let block = Block::new_test_instance();
    let json_block = block.json_representation();

    let result = node
        .runtime()
        .block_on(async { server.client.block_hash(json_block).await.unwrap() });

    assert_eq!(result.hash, block.hash());
}
