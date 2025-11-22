use rsnano_ledger::{
    AnySet, DEV_GENESIS_ACCOUNT, DEV_GENESIS_PUB_KEY, test_helpers::UnsavedBlockLatticeBuilder,
};
use rsnano_node::config::{NodeConfig, NodeFlags};
use rsnano_types::{Amount, DEV_GENESIS_KEY, PrivateKey, WalletId};
use test_helpers::{System, assert_timely_eq2};

#[test]
fn open_create() {
    let mut system = System::new();
    let node = system.make_node();
    assert_eq!(node.wallet_services().wallets.wallet_count(), 1); // it starts out with a default wallet
    let id = WalletId::random();
    assert_eq!(node.wallet_services().wallets.wallet_exists(&id), false);
    node.wallet_services().wallets.create(id);
    assert_eq!(node.wallet_services().wallets.wallet_exists(&id), true);
}

#[test]
fn vote_minimum() {
    let mut system = System::new();
    let node = system.make_node();
    let key1 = PrivateKey::new();
    let key2 = PrivateKey::new();

    let mut lattice = UnsavedBlockLatticeBuilder::new();
    let send1 = lattice.genesis().send(&key1, node.config().vote_minimum);
    node.process(send1.clone());

    let open1 = lattice.account(&key1).receive(&send1);
    node.process(open1.clone());

    let send2 = lattice
        .genesis()
        .send(&key2, node.config().vote_minimum - Amount::raw(1));
    node.process(send2.clone());

    let open2 = lattice.account(&key2).receive(&send2);
    node.process(open2.clone());

    let wallet_id = node.wallet_services().wallets.wallet_ids()[0];
    assert_eq!(
        node.wallet_services()
            .wallet_reps
            .lock()
            .unwrap()
            .voting_reps(),
        0
    );

    node.wallet_services()
        .wallets
        .insert_adhoc2(&wallet_id, &DEV_GENESIS_KEY.raw_key(), false)
        .unwrap();
    node.wallet_services()
        .wallets
        .insert_adhoc2(&wallet_id, &key1.raw_key(), false)
        .unwrap();
    node.wallet_services()
        .wallets
        .insert_adhoc2(&wallet_id, &key2.raw_key(), false)
        .unwrap();
    node.wallet_services()
        .wallet_reps
        .lock()
        .unwrap()
        .compute_reps();
    assert_eq!(
        node.wallet_services()
            .wallet_reps
            .lock()
            .unwrap()
            .voting_reps(),
        2
    );
}

#[test]
fn exists() {
    let mut system = System::new();
    let node = system.make_node();
    let key1 = PrivateKey::new();
    let key2 = PrivateKey::new();
    let wallet_id = node.wallet_services().wallets.wallet_ids()[0];

    assert_eq!(
        node.wallet_services().wallets.exists(&key1.public_key()),
        false
    );
    assert_eq!(
        node.wallet_services().wallets.exists(&key2.public_key()),
        false
    );

    node.wallet_services()
        .wallets
        .insert_adhoc2(&wallet_id, &key1.raw_key(), false)
        .unwrap();
    assert_eq!(
        node.wallet_services().wallets.exists(&key1.public_key()),
        true
    );
    assert_eq!(
        node.wallet_services().wallets.exists(&key2.public_key()),
        false
    );

    node.wallet_services()
        .wallets
        .insert_adhoc2(&wallet_id, &key2.raw_key(), false)
        .unwrap();
    assert_eq!(
        node.wallet_services().wallets.exists(&key1.public_key()),
        true
    );
    assert_eq!(
        node.wallet_services().wallets.exists(&key2.public_key()),
        true
    );
}

#[test]
fn search_receivable() {
    for search_all in [false, true] {
        let mut system = System::new();
        let node = system
            .build_node()
            .config(NodeConfig {
                enable_voting: false,
                ..System::default_config_without_backlog_scan()
            })
            .flags(NodeFlags {
                disable_search_pending: true,
                ..Default::default()
            })
            .finish();
        let wallet_id = node.wallet_services().wallets.wallet_ids()[0];

        node.wallet_services()
            .wallets
            .insert_adhoc2(&wallet_id, &DEV_GENESIS_KEY.raw_key(), false)
            .unwrap();

        let mut lattice = UnsavedBlockLatticeBuilder::new();
        let send = lattice
            .genesis()
            .send(&*DEV_GENESIS_KEY, node.config().receive_minimum);
        node.process(send.clone());

        if search_all {
            node.wallet_services()
                .wallets
                .search_receivable_all()
                .wait()
                .unwrap();
        } else {
            node.wallet_services()
                .wallets
                .search_receivable(&wallet_id)
                .wait()
                .unwrap();
        }
        // Erase the key so the confirmation does not trigger an automatic receive
        node.wallet_services()
            .wallets
            .remove_key(&wallet_id, &DEV_GENESIS_PUB_KEY)
            .unwrap();

        // Now confirm the send block
        node.confirm(send.hash());

        // Re-insert the key
        node.wallet_services()
            .wallets
            .insert_adhoc2(&wallet_id, &DEV_GENESIS_KEY.raw_key(), false)
            .unwrap();

        // Pending search should create the receive block
        assert_eq!(node.ledger_query_services().ledger_arc().block_count(), 2);
        if search_all {
            node.wallet_services()
                .wallets
                .search_receivable_all()
                .wait()
                .unwrap();
        } else {
            node.wallet_services()
                .wallets
                .search_receivable(&wallet_id)
                .wait()
                .unwrap();
        }
        assert_timely_eq2(|| node.balance(&DEV_GENESIS_ACCOUNT), Amount::MAX);
        let receive_hash = node
            .ledger_query_services()
            .ledger_arc()
            .any()
            .account_head(&DEV_GENESIS_ACCOUNT)
            .unwrap();
        let receive = node.block(&receive_hash).unwrap();
        assert_eq!(receive.height(), 3);
        assert_eq!(send.hash(), receive.source().unwrap());
    }
}
