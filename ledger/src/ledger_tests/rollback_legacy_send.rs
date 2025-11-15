use super::new_null_ledger;
use rsnano_types::{Account, Amount, PendingKey, PrivateKey, SavedBlock};

use crate::{
    AnySet, ConfirmedSet, DEV_GENESIS_ACCOUNT, DEV_GENESIS_HASH, Ledger, LedgerInserter, LedgerSet,
    ledger_constants::{DEV_GENESIS_PUB_KEY, LEDGER_CONSTANTS_STUB},
};

#[test]
fn update_vote_weight() {
    let fixture = roll_back_send();
    assert_eq!(fixture.ledger.weight(&DEV_GENESIS_PUB_KEY), Amount::MAX);
}

#[test]
fn update_account_store() {
    let fixture = roll_back_send();

    let account_info = fixture
        .ledger
        .any()
        .get_account(&fixture.ledger.genesis().account())
        .unwrap();

    assert_eq!(account_info.block_count, 1);
    assert_eq!(account_info.head, *DEV_GENESIS_HASH);
    assert_eq!(account_info.balance, LEDGER_CONSTANTS_STUB.genesis_amount);
    assert_eq!(fixture.ledger.account_count(), 1);
}

#[test]
fn remove_from_pending_store() {
    let fixture = roll_back_send();

    let pending = fixture
        .ledger
        .any()
        .get_pending(&PendingKey::new(fixture.destination, fixture.send.hash()));

    assert_eq!(pending, None);
}

#[test]
fn update_confirmation_height_store() {
    let fixture = roll_back_send();

    let conf_height = fixture
        .ledger
        .confirmed()
        .get_conf_info(&DEV_GENESIS_ACCOUNT)
        .unwrap();

    assert_eq!(conf_height.frontier, *DEV_GENESIS_HASH);
    assert_eq!(conf_height.height, 1);
}

#[test]
fn rollback_dependent_blocks_too() {
    let ledger = new_null_ledger();
    let inserter = LedgerInserter::new(&ledger);
    let destination = PrivateKey::from(42);

    let send = inserter.genesis().send(&destination, 1);
    inserter.account(&destination).legacy_open(send.hash());

    // Rollback send block. This requires the rollback of the open block first.
    ledger.roll_back(&send.hash()).unwrap();

    assert_eq!(
        ledger.any().account_balance(&DEV_GENESIS_ACCOUNT),
        Amount::MAX
    );

    assert_eq!(
        ledger.any().account_balance(&destination.account()),
        Amount::ZERO
    );

    assert!(ledger.any().get_account(&destination.account()).is_none());

    let pending = ledger
        .any()
        .get_pending(&PendingKey::new(destination.account(), *DEV_GENESIS_HASH));
    assert_eq!(pending, None);
}

fn roll_back_send() -> Fixture {
    let fixture = create_fixture();
    fixture.ledger.roll_back(&fixture.send.hash()).unwrap();
    fixture
}

struct Fixture {
    ledger: Ledger,
    send: SavedBlock,
    destination: Account,
}

fn create_fixture() -> Fixture {
    let ledger = new_null_ledger();
    let inserter = LedgerInserter::new(&ledger);
    let destination = Account::from(42);
    let amount_sent = Amount::raw(1000);
    let send = inserter.genesis().legacy_send(destination, amount_sent);
    Fixture {
        ledger,
        send,
        destination,
    }
}
