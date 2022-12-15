use crate::{
    legacy_open_block_processor::LegacyOpenBlockProcessor, LedgerConstants,
    LegacyReceiveBlockProcessor, LegacySendBlockProcessor, StateBlockProcessor,
};
use rsnano_core::{
    utils::seconds_since_epoch, valid_change_block_predecessor, validate_message, AccountInfo,
    Amount, Block, BlockDetails, BlockHash, BlockSideband, BlockSubType, ChangeBlock, Epoch,
    MutableBlockVisitor, OpenBlock, ReceiveBlock, SendBlock, StateBlock,
};
use rsnano_store_traits::WriteTransaction;

use super::{Ledger, LedgerObserver, ProcessResult};

pub(crate) struct LedgerProcessor<'a> {
    ledger: &'a Ledger,
    observer: &'a dyn LedgerObserver,
    constants: &'a LedgerConstants,
    txn: &'a mut dyn WriteTransaction,
    pub result: ProcessResult,
}

impl<'a> LedgerProcessor<'a> {
    pub(crate) fn new(
        ledger: &'a Ledger,
        observer: &'a dyn LedgerObserver,
        constants: &'a LedgerConstants,
        txn: &'a mut dyn WriteTransaction,
    ) -> Self {
        Self {
            ledger,
            observer,
            constants,
            txn,
            result: ProcessResult::Progress,
        }
    }
}

impl<'a> MutableBlockVisitor for LedgerProcessor<'a> {
    fn send_block(&mut self, block: &mut SendBlock) {
        self.result = match LegacySendBlockProcessor::new(self.ledger, self.txn, block)
            .process_legacy_send()
        {
            Ok(()) => ProcessResult::Progress,
            Err(res) => res,
        };
    }

    fn receive_block(&mut self, block: &mut ReceiveBlock) {
        self.result = match LegacyReceiveBlockProcessor::new(self.ledger, self.txn, block).process()
        {
            Ok(()) => ProcessResult::Progress,
            Err(res) => res,
        };
    }

    fn open_block(&mut self, block: &mut OpenBlock) {
        self.result = match LegacyOpenBlockProcessor::new(self.ledger, self.txn, block).process() {
            Ok(()) => ProcessResult::Progress,
            Err(res) => res,
        };
    }

    fn change_block(&mut self, block: &mut ChangeBlock) {
        let hash = block.hash();
        let existing = self
            .ledger
            .block_or_pruned_exists_txn(self.txn.txn(), &hash);
        // Have we seen this block before? (Harmless)
        self.result = if existing {
            ProcessResult::Old
        } else {
            ProcessResult::Progress
        };
        if self.result == ProcessResult::Progress {
            // Have we seen the previous block already? (Harmless)
            let previous = match self
                .ledger
                .store
                .block()
                .get(self.txn.txn(), &block.previous())
            {
                Some(b) => b,
                None => {
                    self.result = ProcessResult::GapPrevious;
                    return;
                }
            };
            self.result = if valid_change_block_predecessor(previous.block_type()) {
                ProcessResult::Progress
            } else {
                ProcessResult::BlockPosition
            };
            if self.result == ProcessResult::Progress {
                let account = self
                    .ledger
                    .store
                    .frontier()
                    .get(self.txn.txn(), &block.previous());
                self.result = if account.is_none() {
                    ProcessResult::Fork
                } else {
                    ProcessResult::Progress
                };
                if self.result == ProcessResult::Progress {
                    let account = account.unwrap();
                    let (info, latest_error) =
                        match self.ledger.store.account().get(self.txn.txn(), &account) {
                            Some(i) => (i, false),
                            None => (AccountInfo::default(), true),
                        };
                    debug_assert!(!latest_error);
                    debug_assert!(info.head == block.previous());
                    // Is this block signed correctly (Malformed)
                    self.result = match validate_message(
                        &account.into(),
                        hash.as_bytes(),
                        block.block_signature(),
                    ) {
                        Ok(_) => ProcessResult::Progress,
                        Err(_) => ProcessResult::BadSignature,
                    };
                    if self.result == ProcessResult::Progress {
                        let block_details = BlockDetails::new(
                            Epoch::Epoch0,
                            false, /* unused */
                            false, /* unused */
                            false, /* unused */
                        );
                        // Does this block have sufficient work? (Malformed)
                        self.result = if self.constants.work.difficulty_block(block)
                            >= self
                                .constants
                                .work
                                .threshold2(block.work_version(), &block_details)
                        {
                            ProcessResult::Progress
                        } else {
                            ProcessResult::InsufficientWork
                        };
                        if self.result == ProcessResult::Progress {
                            debug_assert!(validate_message(
                                &account.into(),
                                hash.as_bytes(),
                                block.block_signature()
                            )
                            .is_ok());
                            block.set_sideband(BlockSideband::new(
                                account,
                                BlockHash::zero(),
                                info.balance,
                                info.block_count + 1,
                                seconds_since_epoch(),
                                block_details,
                                Epoch::Epoch0, /* unused */
                            ));
                            self.ledger.store.block().put(self.txn, &hash, block);
                            let balance = self.ledger.balance(self.txn.txn(), &block.previous());
                            self.ledger.cache.rep_weights.representation_add_dual(
                                block.representative(),
                                balance,
                                info.representative,
                                Amount::zero().wrapping_sub(balance),
                            );
                            let new_info = AccountInfo {
                                head: hash,
                                representative: block.representative(),
                                open_block: info.open_block,
                                balance: info.balance,
                                modified: seconds_since_epoch(),
                                block_count: info.block_count + 1,
                                epoch: Epoch::Epoch0,
                            };
                            self.ledger
                                .update_account(self.txn, &account, &info, &new_info);
                            self.ledger
                                .store
                                .frontier()
                                .del(self.txn, &block.previous());
                            self.ledger.store.frontier().put(self.txn, &hash, &account);
                            self.observer.block_added(BlockSubType::Change);
                        }
                    }
                }
            }
        }
    }

    fn state_block(&mut self, block: &mut StateBlock) {
        self.result = match StateBlockProcessor::new(self.ledger, self.txn, block).process() {
            Ok(()) => ProcessResult::Progress,
            Err(res) => res,
        }
    }
}
