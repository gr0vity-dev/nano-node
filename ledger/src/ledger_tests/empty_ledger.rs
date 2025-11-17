use super::new_null_ledger;
use crate::{
    AnySet, ConfirmedSet, DEV_GENESIS_HASH, LedgerSet, ledger_constants::LEDGER_CONSTANTS_STUB,
};
use rsnano_types::{Account, Amount, BlockType, UnixTimestamp};

#[test]
fn account_balance_is_none_for_unknown_account() {
    let ledger = new_null_ledger();
    let balance = ledger.any().account_balance(&Account::ZERO);
    assert_eq!(balance, Amount::ZERO);
}

#[test]
fn get_genesis_block() {
    let ledger = new_null_ledger();

    let genesis = ledger
        .any()
        .get_block(&ledger.genesis().hash())
        .expect("genesis block not found");

    assert_eq!(genesis.block_type(), BlockType::LegacyOpen);
}

#[test]
fn genesis_account_balance() {
    let ledger = new_null_ledger();
    let balance = ledger.any().account_balance(&ledger.genesis().account());
    assert_eq!(balance, Amount::MAX);
}

#[test]
fn genesis_account_info() {
    let ledger = new_null_ledger();

    let account_info = ledger
        .any()
        .get_account(&ledger.genesis().account())
        .expect("genesis account not found");

    // Frontier time should have been updated when genesis balance was added
    assert_eq!(account_info.modified, UnixTimestamp::ZERO);
    assert_eq!(account_info.block_count, 1);
    assert_eq!(account_info.balance, LEDGER_CONSTANTS_STUB.genesis_amount);
}

#[test]
fn genesis_confirmation_height_info() {
    let ledger = new_null_ledger();

    // Genesis block should be confirmed by default
    let conf_info = ledger
        .confirmed()
        .get_conf_info(&ledger.genesis().account())
        .expect("conf height not found");

    assert_eq!(conf_info.height, 1);
    assert_eq!(conf_info.frontier, *DEV_GENESIS_HASH);
}

#[test]
fn empty_ledger_cache() {
    let ledger = new_null_ledger();
    assert_eq!(ledger.account_count(), 1);
    assert_eq!(ledger.block_count(), 1);
    assert_eq!(ledger.confirmed_count(), 1);
}

#[test]
fn genesis_representative() {
    let ledger = new_null_ledger();
    assert_eq!(
        ledger
            .any()
            .representative_block_hash(&ledger.genesis().hash()),
        ledger.genesis().hash()
    );
}

#[test]
fn genesis_vote_weight() {
    let ledger = new_null_ledger();
    assert_eq!(
        ledger.weight(&ledger.genesis().account().into()),
        Amount::MAX
    );
}

#[test]
fn latest_empty() {
    let ledger = new_null_ledger();
    assert_eq!(ledger.any().account_head(&Account::from(1)), None);
}
