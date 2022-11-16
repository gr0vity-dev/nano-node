use crate::{
    core::{Amount, Block, BlockDetails, BlockEnum, Epoch, PendingKey, StateBlock},
    ledger::{datastore::WriteTransaction, DEV_GENESIS_KEY},
    DEV_GENESIS_ACCOUNT,
};

use super::LedgerContext;

#[test]
fn save_block() {
    let ctx = LedgerContext::empty();
    let mut txn = ctx.ledger.rw_txn();

    let (_, receive) = receive_50_raw_into_genesis(&ctx, txn.as_mut());

    let loaded_block = ctx
        .ledger
        .store
        .block()
        .get(txn.txn(), &receive.hash())
        .unwrap();

    let BlockEnum::State(loaded_block) = loaded_block else { panic!("not a state block")};
    assert_eq!(loaded_block, receive);
    assert_eq!(
        loaded_block.sideband().unwrap(),
        receive.sideband().unwrap()
    );
}

#[test]
fn create_sideband() {
    let ctx = LedgerContext::empty();
    let mut txn = ctx.ledger.rw_txn();

    let (_, receive) = receive_50_raw_into_genesis(&ctx, txn.as_mut());

    let sideband = receive.sideband().unwrap();
    assert_eq!(sideband.account, *DEV_GENESIS_ACCOUNT);
    assert_eq!(sideband.height, 3);
    assert_eq!(
        sideband.details,
        BlockDetails::new(Epoch::Epoch0, false, true, false)
    );
}

#[test]
fn update_vote_weight() {
    let ctx = LedgerContext::empty();
    let mut txn = ctx.ledger.rw_txn();

    let (_, receive) = receive_50_raw_into_genesis(&ctx, txn.as_mut());

    assert_eq!(ctx.ledger.weight(&DEV_GENESIS_ACCOUNT), receive.balance());
}

#[test]
fn remove_pending_info() {
    let ctx = LedgerContext::empty();
    let mut txn = ctx.ledger.rw_txn();

    let (send, _) = receive_50_raw_into_genesis(&ctx, txn.as_mut());

    assert_eq!(
        ctx.ledger.store.pending().get(
            txn.txn(),
            &PendingKey::new(*DEV_GENESIS_ACCOUNT, send.hash())
        ),
        None
    );
}

#[test]
fn receive_old_send_block() {
    let ctx = LedgerContext::empty();
    let mut txn = ctx.ledger.rw_txn();
    let send = ctx.process_send_from_genesis(txn.as_mut(), &DEV_GENESIS_ACCOUNT, Amount::new(50));

    let receive = ctx.process_state_receive(txn.as_mut(), &send, &DEV_GENESIS_KEY);

    let sideband = receive.sideband().unwrap();
    assert_eq!(sideband.account, *DEV_GENESIS_ACCOUNT);
    assert_eq!(sideband.height, 3);
    assert_eq!(
        sideband.details,
        BlockDetails::new(Epoch::Epoch0, false, true, false)
    );

    let loaded_block = ctx
        .ledger
        .store
        .block()
        .get(txn.txn(), &receive.hash())
        .unwrap();

    let BlockEnum::State(loaded_block) = loaded_block else { panic!("not a state block")};
    assert_eq!(loaded_block, receive);
    assert_eq!(
        loaded_block.sideband().unwrap(),
        receive.sideband().unwrap()
    );
}

fn receive_50_raw_into_genesis(
    ctx: &LedgerContext,
    txn: &mut dyn WriteTransaction,
) -> (StateBlock, StateBlock) {
    let send = ctx.process_state_send(txn, &DEV_GENESIS_KEY, *DEV_GENESIS_ACCOUNT, Amount::new(50));
    let receive = ctx.process_state_receive(txn, &send, &DEV_GENESIS_KEY);
    (send, receive)
}
