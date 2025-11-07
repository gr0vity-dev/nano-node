use anyhow::anyhow;

use rsnano_ledger::{AnySet, Ledger, LedgerSet};
use rsnano_rpc_messages::{
    AccountHistoryArgs, AccountHistoryResponse, BlockSubTypeDto, BlockTypeDto, HistoryEntry,
    unwrap_bool_or_false, unwrap_u64_or_zero,
};
use rsnano_types::{Account, Block, BlockBase, BlockHash, SavedBlock, UnixTimestamp};

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn account_history(
        &self,
        args: AccountHistoryArgs,
    ) -> anyhow::Result<AccountHistoryResponse> {
        let helper = AccountHistoryHelper::new(&self.node.services.ledger, args);
        helper.account_history()
    }
}

pub(crate) struct AccountHistoryHelper<'a> {
    pub ledger: &'a Ledger,
    pub accounts_to_filter: Vec<Account>,
    pub reverse: bool,
    pub offset: u64,
    pub head: Option<BlockHash>,
    pub requested_account: Option<Account>,
    pub output_raw: bool,
    pub count: u64,
    pub current_block_hash: BlockHash,
    pub account: Account,
    pub include_linked_account: bool,
}

impl<'a> AccountHistoryHelper<'a> {
    fn new(ledger: &'a Ledger, args: AccountHistoryArgs) -> Self {
        Self {
            ledger,
            accounts_to_filter: args.account_filter.unwrap_or_default(),
            reverse: unwrap_bool_or_false(args.reverse),
            offset: unwrap_u64_or_zero(args.offset),
            head: args.head,
            requested_account: args.account,
            output_raw: unwrap_bool_or_false(args.raw),
            count: args.count.into(),
            current_block_hash: BlockHash::ZERO,
            account: Account::ZERO,
            include_linked_account: unwrap_bool_or_false(args.include_linked_account),
        }
    }

    fn initialize(&mut self, any: &impl AnySet) -> anyhow::Result<()> {
        self.current_block_hash = self.hash_of_first_block(any)?;
        self.account = any
            .block_account(&self.current_block_hash)
            .ok_or_else(|| anyhow!(RpcCommandHandler::BLOCK_NOT_FOUND))?;
        Ok(())
    }

    fn hash_of_first_block(&self, any: &impl AnySet) -> anyhow::Result<BlockHash> {
        let hash = if let Some(head) = &self.head {
            *head
        } else {
            let account = self
                .requested_account
                .ok_or_else(|| anyhow!("account argument missing"))?;

            if self.reverse {
                any.get_account(&account)
                    .ok_or_else(|| anyhow!("Account not found"))?
                    .open_block
            } else {
                any.account_head(&account)
                    .ok_or_else(|| anyhow!("Account not found"))?
            }
        };

        Ok(hash)
    }

    pub(crate) fn account_history(mut self) -> anyhow::Result<AccountHistoryResponse> {
        let any = self.ledger.any();
        self.initialize(&any)?;
        let mut history = Vec::new();
        let mut next_block = any.get_block(&self.current_block_hash);
        while let Some(block) = next_block {
            if self.count == 0 {
                break;
            }

            if self.offset > 0 {
                self.offset -= 1;
            } else if let Some(entry) = self.entry_for(&block, &any) {
                history.push(entry);
                self.count -= 1;
            }

            next_block = self.go_to_next_block(&any, &block);
        }

        Ok(self.create_response(history))
    }

    fn go_to_next_block(&mut self, any: &impl AnySet, block: &Block) -> Option<SavedBlock> {
        self.current_block_hash = if self.reverse {
            any.block_successor(&self.current_block_hash)
                .unwrap_or_default()
        } else {
            block.previous()
        };
        any.get_block(&self.current_block_hash)
    }

    fn should_ignore_account(&self, account: &Account) -> bool {
        if self.accounts_to_filter.is_empty() {
            return false;
        }
        !self.accounts_to_filter.contains(account)
    }

    pub(crate) fn entry_for(&self, block: &SavedBlock, any: &impl AnySet) -> Option<HistoryEntry> {
        let mut entry = match &**block {
            Block::LegacySend(b) => {
                let mut entry = empty_entry();
                entry.block_type = Some(BlockTypeDto::Send);
                entry.account = Some(self.account);
                if let Some(amount) = any.block_amount_for(block) {
                    entry.amount = Some(amount);
                } else {
                    entry.destination = Some(self.account);
                    entry.balance = Some(b.balance());
                    entry.previous = Some(b.previous());
                }
                Some(entry)
            }
            Block::LegacyReceive(b) => {
                let mut entry = empty_entry();
                entry.block_type = Some(BlockTypeDto::Receive);
                if let Some(amount) = any.block_amount_for(block) {
                    if let Some(source_account) = any.block_account(&b.source()) {
                        entry.account = Some(source_account);
                    }
                    entry.amount = Some(amount);
                }
                if self.output_raw {
                    entry.source = Some(b.source());
                    entry.previous = Some(b.previous());
                }
                Some(entry)
            }
            Block::LegacyOpen(b) => {
                let mut entry = empty_entry();
                if self.output_raw {
                    entry.block_type = Some(BlockTypeDto::Open);
                    entry.representative = Some(b.representative().into());
                    entry.source = Some(b.source());
                    entry.opened = Some(b.account());
                } else {
                    // Report opens as a receive
                    entry.block_type = Some(BlockTypeDto::Receive);
                }

                if b.source() != self.ledger.constants.genesis_account.into() {
                    if let Some(amount) = any.block_amount_for(block) {
                        entry.account = any.block_account(&b.source());
                        entry.amount = Some(amount);
                    }
                } else {
                    entry.account = Some(self.ledger.constants.genesis_account);
                    entry.amount = Some(self.ledger.constants.genesis_amount);
                }
                Some(entry)
            }
            Block::LegacyChange(b) => {
                if self.output_raw {
                    let mut entry = empty_entry();
                    entry.block_type = Some(BlockTypeDto::Change);
                    entry.representative = Some(b.mandatory_representative().into());
                    entry.previous = Some(b.previous());
                    Some(entry)
                } else {
                    None
                }
            }
            Block::State(b) => {
                let mut entry = empty_entry();
                if self.output_raw {
                    entry.block_type = Some(BlockTypeDto::State);
                    entry.representative = Some(b.representative().into());
                    entry.link = Some(b.link());
                    entry.balance = Some(b.balance());
                    entry.previous = Some(b.previous());
                }

                let balance = b.balance();
                let previous_balance_raw = any.block_balance(&b.previous());
                let previous_balance = previous_balance_raw.unwrap_or_default();
                if !b.previous().is_zero() && previous_balance_raw.is_none() {
                    // If previous hash is non-zero and we can't query the balance, e.g. it's pruned, we can't determine the block type
                    if self.output_raw {
                        entry.subtype = Some(BlockSubTypeDto::Unknown);
                    } else {
                        entry.block_type = Some(BlockTypeDto::Unknown);
                    }
                    Some(entry)
                } else if balance < previous_balance {
                    if self.should_ignore_account(&b.link().into()) {
                        None
                    } else {
                        if self.output_raw {
                            entry.subtype = Some(BlockSubTypeDto::Send);
                        } else {
                            entry.block_type = Some(BlockTypeDto::Send);
                        }
                        entry.account = Some(b.link().into());
                        entry.amount = Some(previous_balance - b.balance());
                        Some(entry)
                    }
                } else if b.link().is_zero() {
                    if self.output_raw && self.accounts_to_filter.is_empty() {
                        entry.subtype = Some(BlockSubTypeDto::Change);
                        Some(entry)
                    } else {
                        None
                    }
                } else if balance == previous_balance && self.ledger.is_epoch_link(&b.link()) {
                    if self.output_raw && self.accounts_to_filter.is_empty() {
                        entry.subtype = Some(BlockSubTypeDto::Epoch);
                        entry.account = self.ledger.epoch_signer(&b.link());
                        Some(entry)
                    } else {
                        None
                    }
                } else {
                    let source_account_opt = any.block_account(&b.link().into());
                    let source_account = source_account_opt.unwrap_or_default();

                    if source_account_opt.is_some() && self.should_ignore_account(&source_account) {
                        None
                    } else {
                        if self.output_raw {
                            entry.subtype = Some(BlockSubTypeDto::Receive);
                        } else {
                            entry.block_type = Some(BlockTypeDto::Receive);
                        }
                        if source_account_opt.is_some() {
                            entry.account = Some(source_account);
                        }
                        entry.amount = Some(balance - previous_balance);
                        Some(entry)
                    }
                }
            }
        };

        if let Some(entry) = &mut entry {
            self.set_common_fields(entry, block, any);
        }
        entry
    }

    fn set_common_fields(&self, entry: &mut HistoryEntry, block: &SavedBlock, any: &impl AnySet) {
        entry.local_timestamp = UnixTimestamp::from(block.timestamp()).as_u64().into();
        entry.height = block.height().into();
        entry.hash = block.hash();
        entry.confirmed = any.confirmed().block_exists(&block.hash()).into();
        if self.output_raw {
            entry.work = Some(block.work());
            entry.signature = Some(block.signature().clone());
        }
        if self.include_linked_account {
            let linked_account = match any.linked_account(block) {
                Some(a) => a.encode_account(),
                None => "0".to_owned(),
            };
            entry.linked_account = Some(linked_account);
        }
    }

    fn create_response(&self, history: Vec<HistoryEntry>) -> AccountHistoryResponse {
        let mut response = AccountHistoryResponse {
            account: self.account,
            history,
            previous: None,
            next: None,
        };

        if !self.current_block_hash.is_zero() {
            if self.reverse {
                response.next = Some(self.current_block_hash);
            } else {
                response.previous = Some(self.current_block_hash);
            }
        }
        response
    }
}

fn empty_entry() -> HistoryEntry {
    HistoryEntry {
        block_type: None,
        amount: None,
        account: None,
        linked_account: None,
        block_account: None,
        local_timestamp: 0.into(),
        height: 0.into(),
        hash: BlockHash::ZERO,
        confirmed: false.into(),
        work: None,
        signature: None,
        representative: None,
        previous: None,
        balance: None,
        source: None,
        opened: None,
        destination: None,
        link: None,
        subtype: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_handler::test_rpc_command;
    use rsnano_rpc_messages::{RpcCommand, RpcError};

    #[test]
    fn history_rpc_call() {
        let cmd = RpcCommand::account_history(
            AccountHistoryArgs::build_for_account(Account::from(42), 3).finish(),
        );

        let result: RpcError = test_rpc_command(cmd);

        assert_eq!(result.error, "Account not found");
    }
}
