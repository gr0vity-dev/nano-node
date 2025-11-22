use rsnano_ledger::{DEV_GENESIS_HASH, DEV_GENESIS_PUB_KEY};
use rsnano_types::{Account, Amount, Block, BlockHash, PrivateKey, StateBlockArgs};
use test_helpers::{System, assert_timely2, setup_rpc_client_and_server};

#[test]
fn unchecked_clear() {
    let mut system = System::new();
    let node = system.make_node();

    let server = setup_rpc_client_and_server(node.clone(), true);

    let key = PrivateKey::new();

    let send1: Block = StateBlockArgs {
        key: &key,
        previous: BlockHash::ZERO,
        representative: *DEV_GENESIS_PUB_KEY,
        balance: Amount::MAX - Amount::raw(1),
        link: Account::ZERO.into(),
        work: node.work_generate_dev(*DEV_GENESIS_HASH),
    }
    .into();

    let _ = node.process_local(send1.clone());

    assert_timely2(|| !node.unchecked().is_empty());

    node.runtime()
        .block_on(async { server.client.unchecked_clear().await.unwrap() });

    assert!(node.unchecked().is_empty());
}
