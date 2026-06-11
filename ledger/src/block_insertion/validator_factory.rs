use rsnano_types::{Account, Block, PendingKey, SavedBlock, UnixMillisTimestamp};

use super::BlockValidator;
use crate::{AnySet, LedgerConstants};

pub(crate) struct BlockValidatorFactory<'a, 'b> {
    any: &'b dyn AnySet,
    constants: &'a LedgerConstants,
    block: &'a Block,
}

impl<'a, 'b> BlockValidatorFactory<'a, 'b> {
    pub(crate) fn new(
        any: &'b dyn AnySet,
        constants: &'a LedgerConstants,
        block: &'a Block,
    ) -> Self {
        Self {
            any,
            constants,
            block,
        }
    }

    pub(crate) fn create_validator(&self) -> BlockValidator<'a> {
        let previous_block = self.load_previous_block();
        let account = self.get_account(&previous_block);
        let account = account.unwrap_or_default();
        let source_block = self.block.source_or_link();
        let source_block_exists = !source_block.is_zero() && self.any.block_exists(&source_block);

        let pending_receive_info = if source_block.is_zero() {
            None
        } else {
            self.any
                .get_pending(&PendingKey::new(account, source_block))
        };

        let existing_block = self.any.get_block(&self.block.hash());

        BlockValidator {
            block: self.block,
            epochs: &self.constants.epochs,
            work: &self.constants.work,
            account,
            existing_block,
            old_account_info: self.any.get_account(&account),
            pending_receive_info,
            any_pending_exists: self.any.receivable_exists(account),
            source_block_exists,
            previous_block,
            now: UnixMillisTimestamp::now(),
        }
    }

    fn get_account(&self, previous: &Option<SavedBlock>) -> Option<Account> {
        match self.block.account_field() {
            Some(account) => Some(account),
            None => previous.as_ref().map(|p| p.account()),
        }
    }

    fn load_previous_block(&self) -> Option<SavedBlock> {
        if !self.block.previous().is_zero() {
            self.any.get_block(&self.block.previous())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Ledger;

    use super::*;
    use rsnano_types::{AccountInfo, BlockHash, Link, PendingInfo, TestBlockBuilder};

    #[test]
    fn block_for_unknown_account() {
        let block = TestBlockBuilder::state().build();
        let ledger = Ledger::new_null_builder().finish();
        let any = ledger.any();
        let validator =
            BlockValidatorFactory::new(&any, &ledger.constants, &block).create_validator();

        assert_eq!(validator.block.hash(), block.hash());
        assert_eq!(validator.epochs, &ledger.constants.epochs);
        assert_eq!(validator.account, block.account_field().unwrap());
        assert!(validator.existing_block.is_none());
        assert_eq!(validator.old_account_info, None);
        assert_eq!(validator.pending_receive_info, None);
        assert_eq!(validator.any_pending_exists, false);
        assert_eq!(validator.source_block_exists, false);
        assert_eq!(validator.previous_block, None);
        assert!(validator.now >= UnixMillisTimestamp::now());
    }

    #[test]
    fn get_account_from_previous_block() {
        let previous = TestBlockBuilder::legacy_send().build_saved();
        let block = TestBlockBuilder::legacy_send()
            .previous(previous.hash())
            .build();
        let ledger = Ledger::new_null_builder().block(&previous).finish();
        let any = ledger.any();
        let validator =
            BlockValidatorFactory::new(&any, &ledger.constants, &block).create_validator();

        assert_eq!(validator.account, previous.account());
    }

    #[test]
    fn block_exists() {
        let block = TestBlockBuilder::state().build_saved();
        let ledger = Ledger::new_null_builder().block(&block).finish();
        let any = ledger.any();
        let validator =
            BlockValidatorFactory::new(&any, &ledger.constants, &block).create_validator();
        assert!(validator.existing_block.is_some());
    }

    #[test]
    fn account_info() {
        let block = TestBlockBuilder::state().build();
        let account_info = AccountInfo::new_test_instance();
        let ledger = Ledger::new_null_builder()
            .account_info(&block.account_field().unwrap(), &account_info)
            .finish();
        let any = ledger.any();
        let validator =
            BlockValidatorFactory::new(&any, &ledger.constants, &block).create_validator();
        assert_eq!(validator.old_account_info, Some(account_info));
    }

    #[test]
    fn pending_receive_info_for_state_block() {
        let block = TestBlockBuilder::state().link(Link::from(42)).build();
        let pending_info = PendingInfo::new_test_instance();
        let ledger = Ledger::new_null_builder()
            .pending(
                &PendingKey::new(block.account_field().unwrap(), BlockHash::from(42)),
                &pending_info,
            )
            .finish();
        let any = ledger.any();
        let validator =
            BlockValidatorFactory::new(&any, &ledger.constants, &block).create_validator();
        assert_eq!(validator.pending_receive_info, Some(pending_info));
    }

    #[test]
    fn pending_receive_info_for_legacy_receive() {
        let previous = TestBlockBuilder::legacy_open().build_saved();
        let account = previous.account();
        let block = TestBlockBuilder::legacy_receive()
            .previous(previous.hash())
            .source(BlockHash::from(42))
            .build();
        let pending_info = PendingInfo::new_test_instance();
        let ledger = Ledger::new_null_builder()
            .block(&previous)
            .pending(
                &PendingKey::new(account, BlockHash::from(42)),
                &pending_info,
            )
            .finish();
        let any = ledger.any();
        let validator =
            BlockValidatorFactory::new(&any, &ledger.constants, &block).create_validator();
        assert_eq!(validator.pending_receive_info, Some(pending_info));
    }

    #[test]
    fn any_pending_exists() {
        let block = TestBlockBuilder::state().build();
        let pending_info = PendingInfo::new_test_instance();
        let ledger = Ledger::new_null_builder()
            .pending(
                &PendingKey::new(block.account_field().unwrap(), BlockHash::from(42)),
                &pending_info,
            )
            .finish();
        let any = ledger.any();
        let validator =
            BlockValidatorFactory::new(&any, &ledger.constants, &block).create_validator();
        assert_eq!(validator.any_pending_exists, true);
    }

    #[test]
    fn source_block_exists() {
        let source = TestBlockBuilder::state().build_saved();
        let block = TestBlockBuilder::state().link(source.hash()).build();
        let ledger = Ledger::new_null_builder().block(&source).finish();
        let any = ledger.any();
        let validator =
            BlockValidatorFactory::new(&any, &ledger.constants, &block).create_validator();
        assert_eq!(validator.source_block_exists, true);
    }

    #[test]
    fn previous_block() {
        let previous = SavedBlock::new_test_instance();
        let block = TestBlockBuilder::state()
            .previous(previous.hash())
            .build_saved();
        let ledger = Ledger::new_null_builder().block(&previous).finish();
        let any = ledger.any();
        let validator =
            BlockValidatorFactory::new(&any, &ledger.constants, &block).create_validator();
        assert_eq!(validator.previous_block, Some(previous));
    }
}
