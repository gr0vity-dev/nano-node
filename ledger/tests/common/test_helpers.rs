use ledger_crate::{block_insertion::BlockInsertInstructions, Ledger};
use rsnano_types::{AccountInfo, Block, BlockSideband, SavedBlock, TestBlockBuilder};
use store_traits::LedgerWriteTxn;

pub fn legacy_open_block_instructions() -> (Block, BlockInsertInstructions) {
    let block = TestBlockBuilder::legacy_open().build();
    let sideband = BlockSideband::new_test_instance();
    let account_info = AccountInfo {
        head: block.hash(),
        open_block: block.hash(),
        ..AccountInfo::new_test_instance()
    };
    let instructions = BlockInsertInstructions {
        account: block.account_field().unwrap(),
        old_account_info: AccountInfo::default(),
        set_account_info: account_info,
        delete_pending: None,
        insert_pending: None,
        set_sideband: sideband,
        is_epoch_block: false,
    };

    (block, instructions)
}

pub fn commit_block_txn(
    ledger: &Ledger,
    txn: Box<dyn LedgerWriteTxn>,
    inserted: bool,
    saved_block: Option<&SavedBlock>,
) {
    let mut hashes = Vec::new();
    if inserted {
        if let Some(block) = saved_block {
            hashes.push(block.hash());
        }
    }
    ledger
        .commit_block_transaction(txn, &hashes)
        .unwrap_or_else(|e| panic!("failed to commit block insertion: {e}"));
}
