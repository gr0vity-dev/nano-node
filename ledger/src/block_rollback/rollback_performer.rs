use rsnano_types::{AccountInfo, BlockHash, SavedBlock};

use crate::{AnySet, BorrowingAnySet, Ledger, LedgerSet};
use store_traits::LedgerWriteTxn;

use super::{
    instructions_executor::RollbackInstructionsExecutor, planner_factory::RollbackPlannerFactory,
    rollback_planner::RollbackStep,
};

pub(crate) struct BlockRollbackPerformer<'a> {
    ledger: &'a Ledger,
    pub txn: &'a mut dyn LedgerWriteTxn,
    pub rolled_back: Vec<SavedBlock>,
}

impl<'a> BlockRollbackPerformer<'a> {
    pub(crate) fn new(ledger: &'a Ledger, txn: &'a mut dyn LedgerWriteTxn) -> Self {
        Self {
            ledger,
            txn,
            rolled_back: Vec::new(),
        }
    }

    /// Rolls back the given block and all of its successor blocks and dependencies
    pub(crate) fn roll_back(&mut self, block_hash: &BlockHash) -> Result<(), RollbackError> {
        // target block + current account head
        let mut targets: Vec<(SavedBlock, SavedBlock)> = Vec::new();

        let target_block = self.load_block(block_hash)?;
        let head_block = self.load_account_head(&target_block)?;
        targets.push((target_block, head_block));

        self.roll_back_impl(&mut targets)
    }

    fn roll_back_impl(
        &mut self,
        targets: &mut Vec<(SavedBlock, SavedBlock)>,
    ) -> Result<(), RollbackError> {
        while let Some((target_block, head_block)) = targets.last_mut() {
            if self.any().block_exists(&target_block.hash()) {
                let step = self.roll_back_head_block(&head_block)?;
                match step {
                    RollbackStep::RollBackBlock(instructions) => {
                        RollbackInstructionsExecutor::new(self.ledger, self.txn, &instructions)
                            .execute();
                        self.rolled_back.push(head_block.clone());
                        if head_block.hash() != target_block.hash() {
                            // The rolled back block wasn't the target, so there are more blocks to
                            // roll back for this account. That's why we have to load the new
                            // head block, which will be rolled back next.
                            *head_block = self.load_account_head(target_block)?;
                        }
                    }
                    RollbackStep::RequestDependencyRollback(dependency_hash) => {
                        let dep_block = self.load_block(&dependency_hash)?;
                        let dep_head = self.load_account_head(&target_block)?;
                        targets.push((dep_block, dep_head));
                    }
                }
            } else {
                targets.pop();
            }
        }
        Ok(())
    }

    fn load_account_head(&self, block: &SavedBlock) -> Result<SavedBlock, RollbackError> {
        let account_info = self.get_account_info(block);
        self.load_block(&account_info.head)
    }

    fn get_account_info(&self, block: &SavedBlock) -> AccountInfo {
        self.any()
            .get_account(&block.account())
            .expect("account not found")
    }

    fn load_block(&self, block_hash: &BlockHash) -> Result<SavedBlock, RollbackError> {
        self.any()
            .get_block(block_hash)
            .ok_or(RollbackError::BlockNotFound)
    }

    fn roll_back_head_block(
        &mut self,
        head_block: &SavedBlock,
    ) -> Result<RollbackStep, RollbackError> {
        let any = self.any();
        let planner =
            RollbackPlannerFactory::new(self.ledger, &any, head_block).create_planner()?;
        planner.roll_back_head_block()
    }

    fn any(&self) -> BorrowingAnySet<'_> {
        BorrowingAnySet {
            constants: &self.ledger.constants,
            store: self.ledger.store_ref(),
            tx: self.txn,
            metrics: self.ledger.iterator_metrics(),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RollbackError {
    /// The block to roll back wasn't found
    BlockNotFound,

    /// A confirmed block must not be rolled back!
    BlockConfirmed,

    PreviousBlockMissing,
    RepresentativeBlockMissing,
    /// Some other component rejected the rollback
    Rejected,
}

impl std::fmt::Display for RollbackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RollbackError::BlockNotFound => f.write_str("Block not found"),
            RollbackError::BlockConfirmed => f.write_str("Cannot roll back confirmed block"),
            RollbackError::PreviousBlockMissing => f.write_str("Previous block missing"),
            RollbackError::RepresentativeBlockMissing => {
                f.write_str("Representative block missing")
            }
            RollbackError::Rejected => f.write_str("Rollback rejected"),
        }
    }
}

impl std::error::Error for RollbackError {}
