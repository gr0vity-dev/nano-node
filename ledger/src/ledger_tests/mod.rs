use std::{
    net::SocketAddrV6,
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};

use rsnano_nullable_lmdb::LmdbEnvironment;
use rsnano_store_lmdb::LmdbAccountStore;
use rsnano_types::{
    Account, AccountInfo, Amount, BlockHash, DEV_GENESIS_KEY, PrivateKey, PublicKey, Root,
    SavedBlock, TestBlockBuilder, UnixMillisTimestamp,
};
use rsnano_utils::stats::Stats;

use crate::{
    AnySet, DEV_GENESIS_HASH, Ledger, LedgerConstants, LedgerInserter, LedgerSet, RepWeightCache,
    ledger_constants::{DEV_GENESIS_BLOCK, DEV_GENESIS_PUB_KEY},
    test_helpers::SavedBlockLatticeBuilder,
};

mod empty_ledger;
mod receivable_iteration;
mod rollback_legacy_change;
mod rollback_legacy_receive;
mod rollback_legacy_send;
mod rollback_state;

#[test]
fn ledger_successor() {
    let ledger = Ledger::new_null();
    let inserter = LedgerInserter::new(&ledger);
    let send = inserter.genesis().send(Account::from(1), 1000);

    assert_eq!(
        ledger
            .any()
            .block_successor_by_qualified_root(&ledger.genesis().qualified_root()),
        Some(ledger.genesis().hash())
    );

    assert_eq!(
        ledger
            .any()
            .block_successor_by_qualified_root(&send.qualified_root()),
        Some(send.hash())
    );
}

#[test]
fn latest_root_empty() {
    let ledger = Ledger::new_null();
    assert_eq!(ledger.any().latest_root(&Account::from(1)), Root::from(1));
}

#[test]
fn latest_root() {
    let ledger = Ledger::new_null();
    let inserter = LedgerInserter::new(&ledger);
    let send = inserter.genesis().send(Account::from(1), 1000);

    assert_eq!(
        ledger.any().latest_root(&ledger.genesis().account()),
        send.hash().into()
    );
}

#[test]
fn send_open_receive_vote_weight() {
    let ledger = Ledger::new_null();
    let inserter = LedgerInserter::new(&ledger);
    let receiver = PrivateKey::from(1);

    let send1 = inserter.genesis().send(&receiver, 50);
    let send2 = inserter.genesis().send(&receiver, 50);
    inserter.account(&receiver).receive(send1.hash());
    inserter.account(&receiver).receive(send2.hash());

    assert_eq!(ledger.weight(&receiver.public_key()), Amount::raw(100));
    assert_eq!(
        ledger.weight(&ledger.genesis().account().into()),
        Amount::MAX - Amount::raw(100)
    );
}

#[test]
fn send_open_receive_rollback() {
    let ledger = Ledger::new_null();
    let inserter = LedgerInserter::new(&ledger);
    let receiver = PrivateKey::from(1);

    let send1 = inserter.genesis().send(&receiver, 50);
    let send2 = inserter.genesis().send(&receiver, 50);
    let open = inserter.account(&receiver).receive(send1.hash());
    let receive = inserter.account(&receiver).receive(send2.hash());

    let rep_account = PublicKey::from(2);
    let change = inserter.genesis().change(rep_account);

    ledger.roll_back(&receive.hash()).unwrap();

    assert_eq!(ledger.weight(&receiver.public_key()), Amount::raw(50));
    assert_eq!(ledger.weight(&DEV_GENESIS_PUB_KEY), Amount::ZERO);
    assert_eq!(ledger.weight(&rep_account), Amount::MAX - Amount::raw(100));

    ledger.roll_back(&open.hash()).unwrap();

    assert_eq!(ledger.weight(&receiver.public_key()), Amount::ZERO);
    assert_eq!(ledger.weight(&DEV_GENESIS_PUB_KEY), Amount::ZERO);
    assert_eq!(ledger.weight(&rep_account), Amount::MAX - Amount::raw(100));

    ledger.roll_back(&change.hash()).unwrap();

    assert_eq!(ledger.weight(&receiver.public_key()), Amount::ZERO);
    assert_eq!(ledger.weight(&rep_account), Amount::ZERO);
    assert_eq!(
        ledger.weight(&DEV_GENESIS_PUB_KEY),
        Amount::MAX - Amount::raw(100)
    );

    ledger.roll_back(&send2.hash()).unwrap();

    assert_eq!(ledger.weight(&receiver.public_key()), Amount::ZERO);
    assert_eq!(ledger.weight(&rep_account), Amount::ZERO);
    assert_eq!(
        ledger.weight(&DEV_GENESIS_PUB_KEY),
        Amount::MAX - Amount::raw(50)
    );

    ledger.roll_back(&send1.hash()).unwrap();

    assert_eq!(ledger.weight(&receiver.public_key()), Amount::ZERO);
    assert_eq!(ledger.weight(&rep_account), Amount::ZERO);
    assert_eq!(ledger.weight(&DEV_GENESIS_PUB_KEY), Amount::MAX);
}

#[test]
fn state_account() {
    let ledger = Ledger::new_null();
    let inserter = LedgerInserter::new(&ledger);
    let send = inserter.genesis().send(Account::from(1), 1000);

    assert_eq!(
        ledger.any().block_account(&send.hash()),
        Some(ledger.genesis().account())
    );
}

#[test]
fn rollbacks_can_be_tracked() {
    let ledger = Ledger::new_null();
    let rollback_tracker = ledger.track_rollbacks();
    let hash = BlockHash::from(123);

    let _ = ledger.roll_back(&hash);

    assert_eq!(rollback_tracker.output(), vec![hash]);
}

mod dependents_confirmed {
    use super::*;
    use crate::AnySet;

    #[test]
    fn genesis_is_confirmed() {
        let ledger = Ledger::new_null();

        assert_eq!(
            ledger
                .any()
                .dependents_confirmed_for_unsaved_block(&ledger.genesis()),
            true
        );
    }

    #[test]
    fn send_dependents_are_confirmed_if_previous_block_is_confirmed() {
        let ledger = Ledger::new_null();
        let inserter = LedgerInserter::new(&ledger);
        let send = inserter.genesis().send(Account::from(1), 1000);

        assert_eq!(
            ledger.any().dependents_confirmed_for_unsaved_block(&send),
            true
        );
    }

    #[test]
    fn send_dependents_are_unconfirmed_if_previous_block_is_unconfirmed() {
        let ledger = Ledger::new_null();
        let inserter = LedgerInserter::new(&ledger);

        inserter.genesis().send(Account::from(1), 1000);
        let send2 = inserter.genesis().send(Account::from(2), 2000);

        assert_eq!(
            ledger.any().dependents_confirmed_for_unsaved_block(&send2),
            false
        );
    }

    #[test]
    fn open_dependents_are_unconfirmed_if_send_block_is_unconfirmed() {
        let ledger = Ledger::new_null();
        let inserter = LedgerInserter::new(&ledger);
        let destination = PrivateKey::from(1);

        let send = inserter.genesis().send(&destination, 1000);
        let open = inserter.account(&destination).receive(send.hash());

        assert_eq!(
            ledger.any().dependents_confirmed_for_unsaved_block(&open),
            false
        );
    }

    #[test]
    fn open_dependents_are_confirmed_if_send_block_is_confirmed() {
        let ledger = Ledger::new_null();
        let inserter = LedgerInserter::new(&ledger);
        let destination = PrivateKey::from(1);

        let send = inserter.genesis().send(&destination, 1000);
        ledger.confirm(send.hash());

        let open = inserter.account(&destination).receive(send.hash());

        assert_eq!(
            ledger.any().dependents_confirmed_for_unsaved_block(&open),
            true
        );
    }

    #[test]
    fn receive_dependents_are_unconfirmed_if_send_block_is_unconfirmed() {
        let ledger = Ledger::new_null();
        let inserter = LedgerInserter::new(&ledger);
        let destination = PrivateKey::from(1);

        let send1 = inserter.genesis().send(&destination, 1000);
        ledger.confirm(send1.hash());

        let send2 = inserter.genesis().send(&destination, 1000);
        let open = inserter.account(&destination).receive(send1.hash());
        ledger.confirm(open.hash());

        let receive = inserter.account(&destination).receive(send2.hash());

        assert_eq!(
            ledger
                .any()
                .dependents_confirmed_for_unsaved_block(&receive),
            false
        );
    }

    #[test]
    fn receive_dependents_are_unconfirmed_if_previous_block_is_unconfirmed() {
        let ledger = Ledger::new_null();
        let inserter = LedgerInserter::new(&ledger);
        let destination = PrivateKey::from(1);

        let send1 = inserter.genesis().send(&destination, 1000);
        ledger.confirm(send1.hash());

        let send2 = inserter.genesis().send(&destination, 1000);
        ledger.confirm(send2.hash());

        inserter.account(&destination).receive(send1.hash());
        let receive = inserter.account(&destination).receive(send2.hash());

        assert_eq!(
            ledger
                .any()
                .dependents_confirmed_for_unsaved_block(&receive),
            false
        );
    }

    #[test]
    fn receive_dependents_are_confirmed_if_previous_block_and_send_block_are_confirmed() {
        let ledger = Ledger::new_null();
        let inserter = LedgerInserter::new(&ledger);
        let destination = PrivateKey::from(1);

        let send1 = inserter.genesis().send(&destination, 1000);
        ledger.confirm(send1.hash());

        let send2 = inserter.genesis().send(&destination, 1000);
        ledger.confirm(send2.hash());

        let open = inserter.account(&destination).receive(send1.hash());
        ledger.confirm(open.hash());

        let receive = inserter.account(&destination).receive(send2.hash());

        assert_eq!(
            ledger
                .any()
                .dependents_confirmed_for_unsaved_block(&receive),
            true
        );
    }
}

#[test]
fn block_confirmed() {
    let ledger = Ledger::new_null();
    let inserter = LedgerInserter::new(&ledger);
    let destination = PrivateKey::from(1);
    let send = inserter.genesis().send(&destination, 1);

    // Must be safe against non-existing blocks
    assert_eq!(ledger.confirmed().block_exists(&BlockHash::from(1)), false);
    assert_eq!(ledger.confirmed().block_exists(&send.hash()), false);

    ledger.confirm(send.hash());

    assert_eq!(ledger.confirmed().block_exists(&send.hash()), true);
}

#[test]
fn ledger_cache() {
    let env = LmdbEnvironment::null_builder().build();
    {
        let accounts = LmdbAccountStore::new(&env).unwrap();
        let mut txn = env.begin_write();

        accounts.put(&mut txn, &1.into(), &AccountInfo::new_test_instance());
        accounts.put(&mut txn, &2.into(), &AccountInfo::new_test_instance());
        accounts.put(&mut txn, &3.into(), &AccountInfo::new_test_instance());
        txn.commit();
    }

    let ledger = Ledger::new(
        env,
        LedgerConstants::live(),
        Amount::ZERO,
        RepWeightCache::new().into(),
        Arc::new(Stats::default()),
        1,
    )
    .unwrap();

    assert_eq!(ledger.account_count(), 3);
}

#[test]
fn is_send_genesis() {
    assert_eq!(DEV_GENESIS_BLOCK.is_send(), false);
}

#[test]
fn sideband_height() {
    let ledger = Ledger::new_null();
    let inserter = LedgerInserter::new(&ledger);
    let dest = PrivateKey::from(42);

    let send = inserter.genesis().legacy_send(&dest, 100);
    let open = inserter.account(&dest).legacy_open(send.hash());
    let change = inserter.genesis().legacy_change(123);
    let state_send = inserter.genesis().send(&dest, 1);
    let receive = inserter.account(&dest).receive(state_send.hash());

    let assert_sideband_height = |hash: &BlockHash, expected_height: u64| {
        let block = ledger.any().get_block(hash).unwrap();
        assert_eq!(block.height(), expected_height);
    };

    assert_sideband_height(&DEV_GENESIS_HASH, 1);
    assert_sideband_height(&send.hash(), 2);
    assert_sideband_height(&open.hash(), 1);
    assert_sideband_height(&receive.hash(), 2);
    assert_sideband_height(&change.hash(), 3);
    assert_sideband_height(&state_send.hash(), 4);
}

#[test]
fn configured_peers_response() {
    let endpoint = "[::ffff:10:0:0:1]:1111".parse::<SocketAddrV6>().unwrap();
    let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
    let ledger = Ledger::new_null_builder().peers([(endpoint, now)]).finish();
    let tx = ledger.store.begin_read();
    assert_eq!(
        ledger.store.peer().iter(&tx).next().unwrap(),
        (endpoint, now)
    );
}

#[test]
fn block_priority() {
    let mut lattice = SavedBlockLatticeBuilder::new();
    lattice.set_now(UnixMillisTimestamp::new(10000));
    let send = lattice.genesis().send(&*DEV_GENESIS_KEY, Amount::nano(500));
    lattice.set_now(UnixMillisTimestamp::new(20000));
    let receive = lattice.genesis().receive(&send);

    let ledger = Ledger::new_null_builder()
        .block(&send)
        .block(&receive)
        .finish();

    assert_eq!(
        ledger
            .any()
            .get_block(&send.hash())
            .unwrap()
            .timestamp()
            .as_u64(),
        10000
    );

    assert_eq!(
        ledger
            .any()
            .get_block(&receive.hash())
            .unwrap()
            .timestamp()
            .as_u64(),
        20000
    );

    let prio = ledger.any().block_priority(&receive);

    assert_eq!(prio.balance, receive.balance());
    assert_eq!(prio.time, send.timestamp().into());
}

#[test]
fn linked_account_for_change_block() {
    let ledger = Ledger::new_null();
    let block = SavedBlock::new_test_change_block();
    assert_eq!(ledger.any().linked_account(&block), None);
}

#[test]
fn linked_account_for_send_block() {
    let ledger = Ledger::new_null();
    let block = SavedBlock::new_test_send_block();
    assert_eq!(
        ledger.any().linked_account(&block),
        Some(block.destination_or_link())
    );
}

#[test]
fn linked_account_for_receive_block() {
    let sender = PrivateKey::from(1);
    let receiver = PrivateKey::from(2);

    let send_block = TestBlockBuilder::state()
        .key(&sender)
        .link(&receiver)
        .is_send()
        .build_saved();

    let receive_block = TestBlockBuilder::state()
        .key(&receiver)
        .link(send_block.hash())
        .is_receive()
        .build_saved();

    let ledger = Ledger::new_null_builder().block(&send_block).finish();
    assert_eq!(
        ledger.any().linked_account(&receive_block),
        Some(sender.account())
    );
}
