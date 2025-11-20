use rsnano_ledger::LedgerSet;
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

#[test]
fn ledger_account_count_handle_matches_ledger_account_count() {
    let node = Node::new_null();
    let account_count = node.production_handles().ledger_account_count();

    assert_eq!(
        account_count.account_count(),
        node.ledger_query_services().ledger.account_count()
    );
}

#[test]
fn ledger_account_balance_handle_matches_ledger_sets() {
    let node = Node::new_null();
    let account = node.network_params.ledger.genesis_account;
    let handle = node.production_handles().ledger_account_balances();
    let ledger = node.ledger_query_services().ledger;
    let ledger_for_confirmed = ledger.clone();

    let (confirmed_balance, confirmed_receivable) =
        handle.confirmed_balance_and_receivable(&account);
    let confirmed_set = ledger_for_confirmed.confirmed();
    assert_eq!(
        confirmed_balance,
        confirmed_set.account_balance(&account)
    );
    assert_eq!(
        confirmed_receivable,
        confirmed_set.account_receivable(&account)
    );

    let (any_balance, any_receivable) = handle.any_balance_and_receivable(&account);
    let any_set = ledger.any();
    assert_eq!(any_balance, any_set.account_balance(&account));
    assert_eq!(any_receivable, any_set.account_receivable(&account));

    assert_eq!(
        handle.account_block_count(&account),
        any_set.get_account(&account).map(|info| info.block_count)
    );
}
