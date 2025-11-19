use rsnano_node::block_processing::BlockSource;
use rsnano_types::{BlockHash, ConfirmationHeightInfo};
use strum::{EnumCount, IntoEnumIterator};
use test_helpers::{System, setup_rpc_client_and_server};

#[test]
fn uncemented_blocks_reports_missing_entries() {
    let mut system = System::new();
    let node = system.make_node();
    let server = setup_rpc_client_and_server(node.clone(), true);

    let ledger = node.ledger_query_services().ledger.clone();
    let genesis_account = node.network_params.ledger.genesis_account;
    let genesis_hash = node.network_params.ledger.genesis_block.hash();

    {
        let mut txn = ledger.store.begin_write();
        ledger.store.confirmation_height().put(
            txn.as_mut(),
            &genesis_account.into(),
            &ConfirmationHeightInfo::new(0, BlockHash::ZERO),
        );
        txn.commit().unwrap();
    }

    let expected_inserts = ledger.block_cache_inserts().to_string();
    let expected_rollbacks = ledger.block_cache_rollbacks().to_string();

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
    assert_eq!(response.cache_inserts, expected_inserts);
    assert_eq!(response.cache_rollbacks, expected_rollbacks);
    assert_eq!(response.duplicate_inserts, "0");

    assert_eq!(response.insert_sources.len(), BlockSource::COUNT);
    let total_from_sources: u64 = response
        .insert_sources
        .iter()
        .map(|entry| entry.inserts.parse::<u64>().unwrap())
        .sum();
    assert_eq!(total_from_sources.to_string(), response.cache_inserts);
    assert_eq!(response.duplicate_sources.len(), BlockSource::COUNT);
    let duplicate_from_sources: u64 = response
        .duplicate_sources
        .iter()
        .map(|entry| entry.inserts.parse::<u64>().unwrap())
        .sum();
    assert_eq!(duplicate_from_sources, 0);
    for source in BlockSource::iter() {
        assert!(
            response
                .insert_sources
                .iter()
                .any(|entry| entry.source == source.as_str())
        );
        assert!(
            response
                .duplicate_sources
                .iter()
                .any(|entry| entry.source == source.as_str())
        );
    }
    assert!(!response.recent_inserts.is_empty());
}
