use rsnano_types::{Amount, Block, BlockHash, PrivateKey, StateBlockArgs};
use test_helpers::{System, assert_timely2, setup_rpc_client_and_server};

#[test]
fn test_unchecked() {
    let mut system = System::new();
    let node = system.build_node().finish();
    let server = setup_rpc_client_and_server(node.clone(), true);

    let key = PrivateKey::new();

    let open = StateBlockArgs {
        key: &key,
        previous: BlockHash::ZERO,
        representative: key.public_key(),
        balance: Amount::raw(1),
        link: key.account().into(),
        work: node.work_generate_dev(key.account()),
    };

    let open2 = StateBlockArgs {
        balance: Amount::raw(2),
        ..open.clone()
    };

    let open = Block::from(open);
    let open2 = Block::from(open2);
    node.process_active(open.clone());
    node.process_active(open2.clone());

    assert_timely2(|| node.unchecked().lock().unwrap().len() == 2);

    let unchecked_dto = node
        .runtime()
        .block_on(async { server.client.unchecked(2).await.unwrap() });

    assert_eq!(unchecked_dto.blocks.len(), 2);
    assert!(unchecked_dto.blocks.contains_key(&open.hash()));
    assert!(unchecked_dto.blocks.contains_key(&open2.hash()));
}
