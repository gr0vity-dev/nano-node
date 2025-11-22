use rsnano_types::{Amount, Block, BlockHash, JsonBlock, PrivateKey, StateBlockArgs};
use test_helpers::{System, assert_timely2, setup_rpc_client_and_server};

#[test]
fn unchecked_get() {
    let mut system = System::new();
    let node = system.build_node().finish();
    let server = setup_rpc_client_and_server(node.clone(), true);

    let key = PrivateKey::new();

    let open: Block = StateBlockArgs {
        key: &key,
        previous: BlockHash::ZERO,
        representative: key.public_key(),
        balance: Amount::raw(1),
        link: key.account().into(),
        work: node.work_generate_dev(key.account()),
    }
    .into();

    node.process_active(open.clone());

    assert_timely2(|| node.unchecked().len() == 1);

    let unchecked_dto = node
        .runtime()
        .block_on(async { server.client.unchecked_get(open.hash()).await.unwrap() });

    let current_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(unchecked_dto.modified_timestamp.inner() <= current_timestamp);

    let json_block: JsonBlock = unchecked_dto.contents;

    assert!(matches!(json_block, JsonBlock::State(_)));

    if let JsonBlock::State(state_block) = json_block {
        assert_eq!(state_block.account, key.account());
        assert_eq!(state_block.previous, BlockHash::ZERO);
        assert_eq!(state_block.representative, key.account());
        assert_eq!(state_block.balance, Amount::raw(1));
        assert_eq!(state_block.link, key.account().into());
    } else {
        panic!("Expected a state block");
    }
}
