use std::sync::{Arc, Mutex};

use rsnano_ledger::{AnySet, BlockError, Ledger};
use rsnano_nullable_clock::SteadyClock;
use rsnano_types::{Account, Block, BlockType, SavedBlock};
use rsnano_utils::stats::{DetailType, StatType, Stats};

use super::state::{BootstrapLogic, PriorityUpResult};
use crate::{
    block_processing::{BlockSource, ProcessedResult},
    services::block_submitter::BlockSubmitter,
};
use rsnano_network::ChannelId;
use tracing::trace;

/// Inspects a processed block and adjusts the bootstrap state accordingly
pub(super) struct BlockInspector {
    state: Arc<Mutex<BootstrapLogic>>,
    ledger: Arc<Ledger>,
    stats: Arc<Stats>,
    clock: Arc<SteadyClock>,
    block_submitter: Arc<BlockSubmitter>,
}

impl BlockInspector {
    pub(super) fn new(
        state: Arc<Mutex<BootstrapLogic>>,
        ledger: Arc<Ledger>,
        stats: Arc<Stats>,
        clock: Arc<SteadyClock>,
        block_submitter: Arc<BlockSubmitter>,
    ) -> Self {
        Self {
            state,
            ledger,
            stats,
            clock,
            block_submitter,
        }
    }

    pub fn inspect(&self, batch: &[ProcessedResult]) {
        let mut state = self.state.lock().unwrap();
        let any = self.ledger.any();
        for result in batch {
            let account = self.get_account(&any, &result.block, &result.saved_block);
            self.inspect_block(&mut state, result, &account);
        }
        self.enqueue_next_blocks(&mut state);
    }

    fn enqueue_next_blocks(&self, state: &mut BootstrapLogic) {
        while let Some((block, query_id)) = state.block_ack_processor.block_queue.next_to_process()
        {
            let block_hash = block.hash();

            trace!(%block_hash, query_id, "Process block");

            match self
                .block_submitter
                // TODO use real channel id
                .submit_bootstrap(block.clone(), ChannelId::LOOPBACK)
            {
                Ok(_) => {
                    state
                        .block_ack_processor
                        .block_queue
                        .enqueued_for_processing(&block_hash);
                }
                Err(err) => {
                    trace!(%block_hash, query_id, ?err, "failed to enqueue bootstrap block");
                    break;
                }
            }
        }
    }

    fn get_account(
        &self,
        any: &dyn AnySet,
        block: &Block,
        saved_block: &Option<SavedBlock>,
    ) -> Account {
        match saved_block {
            Some(b) => b.account(),
            None => block
                .account_field()
                .unwrap_or_else(|| any.block_account(&block.previous()).unwrap_or_default()),
        }
    }

    /// Inspects a block that has been processed by the block processor
    /// - Marks an account as blocked if the result code is gap source as there is no reason request additional blocks for this account until the dependency is resolved
    /// - Marks an account as forwarded if it has been recently referenced by a block that has been inserted.
    fn inspect_block(
        &self,
        state: &mut BootstrapLogic,
        result: &ProcessedResult,
        account: &Account,
    ) {
        let hash = result.block.hash();

        match result.status {
            Ok(()) => {
                // Progress blocks from live traffic don't need further bootstrapping
                if result.source != BlockSource::Live {
                    let saved_block = result.saved_block.clone().unwrap();
                    let account = saved_block.account();
                    // If we've inserted any block in to an account, unmark it as blocked
                    if state.candidate_accounts.unblock(account, None) {
                        self.stats
                            .inc(StatType::BootstrapAccountSets, DetailType::Unblock);
                        self.stats.inc(
                            StatType::BootstrapAccountSets,
                            DetailType::PriorityUnblocked,
                        );
                    }

                    match state.candidate_accounts.priority_up(&account) {
                        PriorityUpResult::Updated => {
                            self.stats
                                .inc(StatType::BootstrapAccountSets, DetailType::Prioritize);
                        }
                        PriorityUpResult::Inserted => {
                            self.stats
                                .inc(StatType::BootstrapAccountSets, DetailType::Prioritize);
                            self.stats
                                .inc(StatType::BootstrapAccountSets, DetailType::PriorityInsert);
                        }
                        PriorityUpResult::AccountBlocked => {
                            self.stats
                                .inc(StatType::BootstrapAccountSets, DetailType::PrioritizeFailed);
                        }
                        PriorityUpResult::InvalidAccount => {}
                    }

                    if saved_block.is_send() {
                        let destination = saved_block.destination().unwrap();
                        // Unblocking automatically inserts account into priority set
                        if state.candidate_accounts.unblock(destination, Some(hash)) {
                            self.stats
                                .inc(StatType::BootstrapAccountSets, DetailType::Unblock);
                            self.stats.inc(
                                StatType::BootstrapAccountSets,
                                DetailType::PriorityUnblocked,
                            );
                        } else {
                            if matches!(
                                state.candidate_accounts.priority_up(&destination),
                                PriorityUpResult::Inserted | PriorityUpResult::Updated
                            ) {
                                self.stats.inc(
                                    StatType::BootstrapAccountSets,
                                    DetailType::PriorityInsert,
                                );
                            } else {
                                self.stats.inc(
                                    StatType::BootstrapAccountSets,
                                    DetailType::PrioritizeFailed,
                                );
                            };
                        }
                    }
                }

                let info = state.block_ack_processor.block_queue.processed(&hash);
                if let Some(account) = info.account {
                    if info.was_last {
                        state.candidate_accounts.reset_last_request(&account);
                    }
                }
            }
            Err(error) => {
                state
                    .block_ack_processor
                    .block_queue
                    .processing_failed(&hash);

                match error {
                    BlockError::GapSource => {
                        // Prevent malicious live traffic from filling up the blocked set
                        if result.source == BlockSource::Bootstrap {
                            let source = result.block.source_or_link();

                            if !account.is_zero() && !source.is_zero() {
                                // Mark account as blocked because it is missing the source block
                                let blocked = state.candidate_accounts.block(
                                    *account,
                                    source,
                                    self.clock.now(),
                                );
                                if blocked {
                                    self.stats.inc(
                                        StatType::BootstrapAccountSets,
                                        DetailType::PriorityEraseBlock,
                                    );
                                    self.stats
                                        .inc(StatType::BootstrapAccountSets, DetailType::Block);
                                } else {
                                    self.stats.inc(
                                        StatType::BootstrapAccountSets,
                                        DetailType::BlockFailed,
                                    );
                                }
                            }
                        }
                    }
                    BlockError::GapPrevious => {
                        // Prevent live traffic from evicting accounts from the priority list
                        if result.source == BlockSource::Live
                            && !state.candidate_accounts.priority_half_full()
                            && !state.candidate_accounts.blocked_half_full()
                        {
                            if result.block.block_type() == BlockType::State {
                                let account = result.block.account_field().unwrap();
                                if matches!(
                                    state.candidate_accounts.priority_up(&account),
                                    PriorityUpResult::Updated | PriorityUpResult::Inserted
                                ) {
                                    self.stats.inc(
                                        StatType::BootstrapAccountSets,
                                        DetailType::PriorityInsert,
                                    );
                                } else {
                                    self.stats.inc(
                                        StatType::BootstrapAccountSets,
                                        DetailType::PrioritizeFailed,
                                    );
                                }
                            }
                        }
                    }
                    BlockError::GapEpochOpenPending => {
                        // Epoch open blocks for accounts that don't have any pending blocks yet
                        if state.candidate_accounts.priority_erase(account) {
                            self.stats
                                .inc(StatType::BootstrapAccountSets, DetailType::PriorityErase);
                        }
                    }
                    _ => {
                        // No need to handle other cases
                        // TODO: If we receive blocks that are invalid (bad signature, fork, etc.),
                        // we should penalize the peer that sent them
                    }
                }
            }
        }
    }
}
