use super::new_null_ledger;
use rsnano_types::{Account, Amount};

use crate::{AnySet, LedgerInserter, ledger_constants::DEV_GENESIS_PUB_KEY};

#[test]
fn rollback_dependent_blocks_too() {
    let ledger = new_null_ledger();
    let inserter = LedgerInserter::new(&ledger);

    let change = inserter.genesis().legacy_change(123);
    let send = inserter.genesis().legacy_send(Account::from(1), 100);

    ledger.roll_back(&change.hash()).unwrap();

    assert_eq!(ledger.any().get_block(&send.hash()), None);
    assert_eq!(ledger.weight(&DEV_GENESIS_PUB_KEY), Amount::MAX);
}
