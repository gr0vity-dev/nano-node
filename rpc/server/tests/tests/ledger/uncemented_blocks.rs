use rsnano_types::{BlockHash, ConfirmationHeightInfo};
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn uncemented_blocks_reports_missing_entries() {
    let mut system = System::new();
    let node = system.make_node();
    let server = setup_rpc_client_and_server(node.clone(), true);

    let ledger = &node.ledger_services.ledger;
    let genesis_account = node.network_params.ledger.genesis_account;
    let genesis_hash = node.network_params.ledger.genesis_block.hash();

    {
        let mut txn = ledger.store_ref().begin_write();
        ledger.store.confirmation_height().put(
            txn.as_mut(),
            &genesis_account.into(),
            &ConfirmationHeightInfo::new(0, BlockHash::ZERO),
        );
        txn.commit().unwrap();
    }

    let response = node.runtime.block_on(async {
        server
            .client
            .uncemented_blocks(Default::default())
            .await
            .unwrap()
    });

    assert_eq!(response.accounts.len(), 1);
    let entry = &response.accounts[0];
    assert_eq!(entry.account, genesis_account.encode_account());
    assert_eq!(entry.head, genesis_hash.to_string());
    assert_eq!(entry.missing_count, "1");
}
