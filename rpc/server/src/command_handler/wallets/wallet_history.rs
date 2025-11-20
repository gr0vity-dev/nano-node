use rsnano_rpc_messages::{AccountHistoryArgs, HistoryEntry, WalletHistoryArgs, WalletHistoryResponse};
use rsnano_types::{BlockHash, UnixTimestamp};

use crate::command_handler::{RpcCommandHandler, ledger::AccountHistoryHelper};

impl RpcCommandHandler {
    pub(crate) fn wallet_history(
        &self,
        args: WalletHistoryArgs,
    ) -> anyhow::Result<WalletHistoryResponse> {
        let modified_since: UnixTimestamp = args.modified_since.unwrap_or_default().inner().into();
        let accounts = self
            .wallet_services
            .wallets
            .get_accounts_of_wallet(&args.wallet)?;
        let mut entries: Vec<HistoryEntry> = Vec::new();

        for account in accounts {
            if let Some(info) = self.ledger_queries.account_info(&account) {
                let mut timestamp = info.modified;
                let mut hash = info.head;

                while timestamp >= modified_since && !hash.is_zero() {
                    if let Some(block) = self.ledger_queries.get_block(&hash) {
                        timestamp = block.timestamp().into();

                        let helper = AccountHistoryHelper::new(
                            self.ledger_queries.clone(),
                            AccountHistoryArgs::new(account, u64::MAX),
                        );

                        let entry = helper.entry_for(&block);

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
