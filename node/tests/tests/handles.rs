use rsnano_node::Node;

#[test]
fn ledger_info_handle_matches_ledger_metadata() {
    let node = Node::new_null();
    let ledger_info = node.production_handles().ledger_info();

    let expected_version = node.ledger_query_services().ledger.version();
    assert_eq!(ledger_info.store_version(), expected_version);

    let vendor = ledger_info.store_vendor();
    assert!(
        vendor.starts_with("lmdb"),
        "expected vendor to start with lmdb, got {vendor}"
    );
}

#[test]
fn ledger_counts_handle_matches_ledger_counters() {
    let node = Node::new_null();
    let ledger_counts = node.production_handles().ledger_counts();

    assert_eq!(
        ledger_counts.block_count(),
        node.ledger_query_services().ledger.block_count()
    );
    assert_eq!(
        ledger_counts.confirmed_count(),
        node.ledger_query_services().ledger.confirmed_count()
    );
}
