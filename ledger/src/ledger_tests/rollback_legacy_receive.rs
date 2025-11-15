use super::new_null_ledger;
use rsnano_types::{Amount, PendingKey, PrivateKey, SavedBlock};

use crate::{AnySet, Ledger, LedgerInserter, LedgerSet};

#[test]
fn clear_successor() {
    let fixture = create_fixture();

    fixture.ledger.roll_back(&fixture.receive.hash()).unwrap();

    assert_eq!(
        fixture.ledger.any().block_successor(&fixture.open.hash()),
        None
    );
}

#[test]
fn update_account_info() {
    let fixture = create_fixture();

    fixture.ledger.roll_back(&fixture.receive.hash()).unwrap();

    let account_info = fixture
        .ledger
        .any()
        .get_account(&fixture.receive.account())
        .unwrap();

    assert_eq!(account_info.head, fixture.open.hash());
    assert_eq!(account_info.block_count, 1);
    assert_eq!(account_info.balance, fixture.open.balance());
}

#[test]
fn rollback_pending_info() {
    let fixture = create_fixture();

    fixture.ledger.roll_back(&fixture.receive.hash()).unwrap();

    let pending = fixture
        .ledger
        .any()
        .get_pending(&PendingKey::new(
            fixture.receive.account(),
            fixture.receive.source_or_link(),
        ))
        .unwrap();

    assert_eq!(pending.source, fixture.ledger.genesis().account());
    assert_eq!(pending.amount, fixture.amount_received);
}

#[test]
fn rollback_vote_weight() {
    let fixture = create_fixture();

    fixture.ledger.roll_back(&fixture.receive.hash()).unwrap();

    assert_eq!(
        fixture.ledger.weight(&fixture.receive.account().into()),
        fixture.amount_opened
    );
}

struct Fixture {
    ledger: Ledger,
    open: SavedBlock,
    receive: SavedBlock,
    amount_opened: Amount,
    amount_received: Amount,
}

fn create_fixture() -> Fixture {
    let ledger = new_null_ledger();
    let inserter = LedgerInserter::new(&ledger);
    let destination = PrivateKey::from(42);
    let amount_opened = Amount::raw(500);
    let amount_received = Amount::raw(1000);
    let send1 = inserter.genesis().legacy_send(&destination, amount_opened);
    let send2 = inserter
        .genesis()
        .legacy_send(&destination, amount_received);
    let open = inserter.account(&destination).legacy_open(send1.hash());
    let receive = inserter.account(&destination).legacy_receive(send2.hash());

    Fixture {
        ledger,
        open,
        receive,
        amount_received,
        amount_opened,
    }
}
