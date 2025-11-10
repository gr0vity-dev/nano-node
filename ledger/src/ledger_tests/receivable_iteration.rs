use crate::{AnySet, Ledger};
use rsnano_types::{Account, BlockHash, PendingInfo, PendingKey};

#[test]
fn empty() {
    let ledger = Ledger::new_null();
    let any = ledger.any();

    let mut iterator = any.account_receivable_upper_bound(Account::ZERO, BlockHash::ZERO);

    assert_eq!(iterator.next(), None);

    let any = ledger.any();
    let mut iterator = any.receivable_upper_bound(Account::ZERO);
    assert_eq!(iterator.next(), None);
}

#[test]
fn reveivable_upper_bound_for_given_account() {
    let ledger = Ledger::new_null();
    let mut txn = ledger.store.begin_write();

    let account = Account::from(100);
    let hash = BlockHash::from(200);
    let key_0 = PendingKey::new(1.into(), 1.into());
    let key_1 = PendingKey::new(account, hash);
    let key_2 = PendingKey::new(account, 300.into());
    let key_3 = PendingKey::new(200.into(), 1.into());
    let pending = PendingInfo::new_test_instance();
    ledger.store.pending().put(&mut txn, &key_0, &pending);
    ledger.store.pending().put(&mut txn, &key_1, &pending);
    ledger.store.pending().put(&mut txn, &key_2, &pending);
    ledger.store.pending().put(&mut txn, &key_3, &pending);
    txn.commit();
    let any = ledger.any();

    // exact match
    let mut iterator = any.account_receivable_upper_bound(account, hash);
    assert_eq!(iterator.next(), Some((key_2.clone(), pending.clone())));
    assert_eq!(iterator.next(), None);

    // find higher
    let mut iterator = any.account_receivable_upper_bound(account, BlockHash::from(0));
    assert_eq!(iterator.next(), Some((key_1.clone(), pending.clone())));
    assert_eq!(iterator.next(), Some((key_2.clone(), pending.clone())));
    assert_eq!(iterator.next(), None);

    // too high
    let mut iterator = any.account_receivable_upper_bound(account, BlockHash::from(301));
    assert_eq!(iterator.next(), None);
}

#[test]
fn reveivable_upper_bound() {
    let ledger = Ledger::new_null();
    let mut txn = ledger.store.begin_write();

    let key_1 = PendingKey::new(100.into(), 200.into());
    let key_2 = PendingKey::new(100.into(), 300.into());
    let key_3 = PendingKey::new(200.into(), 1.into());
    let pending = PendingInfo::new_test_instance();
    ledger.store.pending().put(&mut txn, &key_1, &pending);
    ledger.store.pending().put(&mut txn, &key_2, &pending);
    ledger.store.pending().put(&mut txn, &key_3, &pending);
    txn.commit();
    let any = ledger.any();

    // same account
    let mut iterator = any.receivable_upper_bound(100.into());
    assert_eq!(iterator.next(), Some((key_3.clone(), pending.clone())));
    assert_eq!(iterator.next(), None);

    // lower
    let mut iterator = any.receivable_upper_bound(99.into());
    assert_eq!(iterator.next(), Some((key_1.clone(), pending.clone())));
    assert_eq!(iterator.next(), Some((key_2.clone(), pending.clone())));
    assert_eq!(iterator.next(), None);

    // too high
    let mut iterator = any.receivable_upper_bound(200.into());
    assert_eq!(iterator.next(), None);
}

#[test]
fn reveivable_any() {
    let ledger = Ledger::new_null();
    let mut txn = ledger.store.begin_write();

    let key = PendingKey::new(100.into(), 200.into());
    let pending = PendingInfo::new_test_instance();
    ledger.store.pending().put(&mut txn, &key, &pending);
    txn.commit();

    let any = ledger.any();
    assert_eq!(any.receivable_exists(100.into()), true);
    assert_eq!(any.receivable_exists(99.into()), false);
    assert_eq!(any.receivable_exists(101.into()), false);
}
