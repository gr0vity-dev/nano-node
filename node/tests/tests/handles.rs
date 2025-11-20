use rsnano_ledger::{AnySet, ConfirmedSet, LedgerSet};
use rsnano_node::Node;
use rsnano_types::{BlockHash, Epoch, PendingKey, SavedBlock};

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

#[test]
fn ledger_work_threshold_handle_matches_ledger_constants() {
    let node = Node::new_null();
    let handle = node.production_handles().ledger_work_thresholds();

    assert_eq!(
        handle.threshold_base(),
        node.ledger_query_services().ledger.work_thresholds().threshold_base()
    );
}

#[test]
fn ledger_state_check_handle_matches_ledger_queries() {
    let node = Node::new_null();
    let handle = node.production_handles().ledger_state_checks();
    let ledger = node.ledger_query_services().ledger;
    let genesis_hash = node.network_params.ledger.genesis_block.hash();
    let genesis_account = node.network_params.ledger.genesis_account;

    assert!(handle.block_exists(&genesis_hash));
    assert_eq!(
        handle.account_balance(&genesis_account),
        ledger.any().account_balance(&genesis_account)
    );

    if let Some(link) = node.network_params.ledger.epochs.link(Epoch::Epoch1) {
        assert!(handle.is_epoch_link(link));
    }
}

#[test]
fn ledger_query_handle_matches_ledger_reads() {
    let node = Node::new_null();
    let handle = node.production_handles().ledger_queries();
    let ledger = node.ledger_query_services().ledger;
    let genesis_account = node.network_params.ledger.genesis_account;
    let genesis_hash = node.network_params.ledger.genesis_block.hash();

    let account_info = handle.account_info(&genesis_account).unwrap();
    let ledger_account_info = ledger.any().get_account(&genesis_account).unwrap();
    assert_eq!(account_info.head, ledger_account_info.head);

    let conf_info = handle.confirmation_height_info(&genesis_account).unwrap();
    let ledger_conf_info = ledger
        .confirmed()
        .get_conf_info(&genesis_account)
        .unwrap();
    assert_eq!(conf_info.height, ledger_conf_info.height);

    assert_eq!(
        handle.representative_block_hash(&ledger_account_info.head),
        ledger.any().representative_block_hash(&ledger_account_info.head)
    );

    assert_eq!(
        handle.block_balance(&genesis_hash),
        ledger.any().block_balance(&genesis_hash)
    );

    let saved_block: SavedBlock = handle.get_block(&genesis_hash).unwrap();
    assert_eq!(saved_block.hash(), genesis_hash);

    let detailed = handle.detailed_block(&genesis_hash).unwrap();
    assert_eq!(detailed.block.hash(), genesis_hash);

    assert_eq!(
        handle.block_successor(&genesis_hash),
        ledger.any().block_successor(&genesis_hash)
    );

    assert!(handle.confirmed_block_exists(&genesis_hash));
    assert_eq!(
        handle.account_head(&genesis_account),
        ledger.any().account_head(&genesis_account)
    );
}

#[test]
fn ledger_query_handle_receivable_upper_bound_matches_iterator() {
    let node = Node::new_null();
    let account = node.network_params.ledger.genesis_account;
    let start = BlockHash::ZERO;
    let handle_iter = node
        .production_handles()
        .ledger_queries()
        .receivable_upper_bound(account, start)
        .into_iter()
        .collect::<Vec<_>>();
    let ledger_iter = node
        .ledger_query_services()
        .ledger
        .any()
        .account_receivable_upper_bound(account, start)
        .collect::<Vec<_>>();

    assert_eq!(handle_iter, ledger_iter);
}

#[test]
fn ledger_query_handle_pending_from_matches_iterator() {
    let node = Node::new_null();
    let start = PendingKey::new(
        node.network_params.ledger.genesis_account,
        node.network_params.ledger.genesis_block.hash(),
    );
    let handle_iter = node
        .production_handles()
        .ledger_queries()
        .pending_from(start)
        .into_iter()
        .collect::<Vec<_>>();
    let ledger_iter = node
        .ledger_query_services()
        .ledger
        .any()
        .iter_pending_range(start..)
        .collect::<Vec<_>>();

    assert_eq!(handle_iter, ledger_iter);
}
