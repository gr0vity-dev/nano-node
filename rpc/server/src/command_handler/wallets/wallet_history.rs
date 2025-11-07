use rsnano_ledger::{AnySet, LedgerSet};
use rsnano_rpc_messages::{HistoryEntry, WalletHistoryArgs, WalletHistoryResponse};
use rsnano_types::{Account, BlockHash, UnixTimestamp};

use crate::command_handler::{RpcCommandHandler, ledger::AccountHistoryHelper};

impl RpcCommandHandler {
    pub(crate) fn wallet_history(
        &self,
        args: WalletHistoryArgs,
    ) -> anyhow::Result<WalletHistoryResponse> {
        let modified_since: UnixTimestamp = args.modified_since.unwrap_or_default().inner().into();
        let accounts = self
            .node
            .services
            .wallets
            .get_accounts_of_wallet(&args.wallet)?;
        let mut entries: Vec<HistoryEntry> = Vec::new();
        let any = self.node.services.ledger.any();

        for account in accounts {
            if let Some(info) = any.get_account(&account) {
                let mut timestamp = info.modified;
                let mut hash = info.head;

                while timestamp >= modified_since && !hash.is_zero() {
                    if let Some(block) = any.get_block(&hash) {
                        timestamp = block.timestamp().into();

                        let helper = AccountHistoryHelper {
                            ledger: &self.node.services.ledger,
                            accounts_to_filter: Vec::new(),
                            reverse: false,
                            offset: 0,
                            head: None,
                            requested_account: Some(account),
                            output_raw: false,
                            count: u64::MAX,
                            current_block_hash: BlockHash::ZERO,
                            account: Account::ZERO,
                            include_linked_account: false,
                        };

                        let entry = helper.entry_for(&block, &any);

                        if let Some(mut entry) = entry {
                            entry.block_account = Some(account);
                            entries.push(entry);
                        }

                        hash = block.previous();
                    } else {
                        hash = BlockHash::ZERO
                    }
                }
            }
        }

        entries.sort_by(|a, b| b.local_timestamp.cmp(&a.local_timestamp));
        Ok(WalletHistoryResponse::new(entries))
    }
}
