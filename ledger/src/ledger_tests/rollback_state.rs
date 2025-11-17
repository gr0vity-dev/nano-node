use super::new_null_ledger;
use crate::{
    AnySet, DEV_GENESIS_ACCOUNT, DEV_GENESIS_HASH, LedgerInserter, LedgerSet,
    ledger_constants::DEV_GENESIS_PUB_KEY,
};
use rsnano_types::{Account, Amount, Epoch, PendingInfo, PendingKey, PrivateKey, PublicKey};

#[test]
fn rollback_send() {
    let ledger = new_null_ledger();
    let inserter = LedgerInserter::new(&ledger);
    let send = inserter.genesis().send(Account::from(1), 100);

    ledger.roll_back(&send.hash()).unwrap();
    let any = ledger.any();

    assert_eq!(any.block_exists(&send.hash()), false);
    assert_eq!(any.account_balance(&DEV_GENESIS_ACCOUNT), Amount::MAX);
    assert_eq!(ledger.weight(&DEV_GENESIS_PUB_KEY), Amount::MAX);
    assert_eq!(
        any.get_pending(&PendingKey::new(*DEV_GENESIS_ACCOUNT, send.hash())),
        None
    );
    assert_eq!(any.block_successor(&DEV_GENESIS_HASH), None);
}

#[test]
fn rollback_receive() {
    let ledger = new_null_ledger();
    let inserter = LedgerInserter::new(&ledger);

    let amount_sent = Amount::raw(50);
    let send = inserter
        .genesis()
        .send(ledger.genesis().account(), amount_sent);
    let receive = inserter.genesis().receive(send.hash());

    ledger.roll_back(&receive.hash()).unwrap();
    let any = ledger.any();

    assert_eq!(any.block_exists(&receive.hash()), false);
    assert_eq!(
        any.account_balance(&DEV_GENESIS_ACCOUNT),
        Amount::MAX - amount_sent
    );
    assert_eq!(
        ledger.weight(&DEV_GENESIS_PUB_KEY),
        Amount::MAX - amount_sent
    );
    assert_eq!(
        any.get_pending(&PendingKey::new(*DEV_GENESIS_ACCOUNT, send.hash())),
        Some(PendingInfo {
            source: *DEV_GENESIS_ACCOUNT,
            amount: amount_sent,
            epoch: Epoch::Epoch0
        })
    );
    assert_eq!(any.block_successor(&send.hash()), None);
}

#[test]
fn rollback_received_send() {
    let ledger = new_null_ledger();
    let inserter = LedgerInserter::new(&ledger);
    let destination = PrivateKey::from(1);

    let send = inserter.genesis().send(&destination, 1);
    let open = inserter.account(&destination).receive(send.hash());

    ledger.roll_back(&send.hash()).unwrap();

    let any = ledger.any();
    assert_eq!(
        any.get_pending(&PendingKey::new(*DEV_GENESIS_ACCOUNT, send.hash())),
        None
    );
    assert_eq!(any.block_exists(&send.hash()), false);
    assert_eq!(any.block_exists(&open.hash()), false);
    assert_eq!(any.account_balance(&DEV_GENESIS_ACCOUNT), Amount::MAX);
    assert_eq!(ledger.weight(&DEV_GENESIS_PUB_KEY), Amount::MAX);
    assert_eq!(any.account_balance(&destination.account()), Amount::ZERO);
}

#[test]
fn rollback_rep_change() {
    let ledger = new_null_ledger();
    let inserter = LedgerInserter::new(&ledger);
    let representative = PublicKey::from(1);

    let change = inserter.genesis().change(representative);

    ledger.roll_back(&change.hash()).unwrap();
    let any = ledger.any();

    assert_eq!(any.block_exists(&change.hash()), false);
    assert_eq!(any.account_balance(&DEV_GENESIS_ACCOUNT), Amount::MAX);
    assert_eq!(ledger.weight(&DEV_GENESIS_PUB_KEY), Amount::MAX);
    assert_eq!(ledger.weight(&representative), Amount::ZERO);
}

#[test]
fn rollback_open() {
    let ledger = new_null_ledger();
    let inserter = LedgerInserter::new(&ledger);
    let destination = PrivateKey::from(1);

    let amount_sent = Amount::raw(50);
    let send = inserter.genesis().send(&destination, amount_sent);
    let open = inserter.account(&destination).receive(send.hash());

    ledger.roll_back(&open.hash()).unwrap();
    let any = ledger.any();

    assert_eq!(any.block_exists(&open.hash()), false);
    assert_eq!(any.account_balance(&destination.account()), Amount::ZERO);
    assert_eq!(
        ledger.weight(&DEV_GENESIS_PUB_KEY),
        Amount::MAX - amount_sent
    );
    assert_eq!(
        any.get_pending(&PendingKey::new(destination.account(), send.hash()))
            .unwrap(),
        PendingInfo {
            source: *DEV_GENESIS_ACCOUNT,
            amount: Amount::raw(50),
            epoch: Epoch::Epoch0
        }
    );
}

#[test]
fn rollback_send_with_rep_change() {
    let ledger = new_null_ledger();
    let inserter = LedgerInserter::new(&ledger);

    let representative = PublicKey::from(1);
    let send = inserter
        .genesis()
        .send_and_change(Account::from(42), 1000, representative);

    ledger.roll_back(&send.hash()).unwrap();
    let any = ledger.any();

    assert_eq!(any.block_exists(&send.hash()), false);
    assert_eq!(any.account_balance(&DEV_GENESIS_ACCOUNT), Amount::MAX);
    assert_eq!(ledger.weight(&DEV_GENESIS_PUB_KEY), Amount::MAX);
    assert_eq!(ledger.weight(&representative), Amount::ZERO);
}

#[test]
fn rollback_receive_with_rep_change() {
    let ledger = new_null_ledger();
    let inserter = LedgerInserter::new(&ledger);

    let representative = PublicKey::from(1);
    let send = inserter.genesis().send(ledger.genesis().account(), 1);
    let receive = inserter
        .genesis()
        .receive_and_change(send.hash(), representative);

    ledger.roll_back(&receive.hash()).unwrap();
    let any = ledger.any();

    assert_eq!(any.block_exists(&receive.hash()), false);
    assert_eq!(any.account_balance(&DEV_GENESIS_ACCOUNT), send.balance());
    assert_eq!(ledger.weight(&DEV_GENESIS_PUB_KEY), send.balance());
    assert_eq!(ledger.weight(&representative), Amount::ZERO);
}
