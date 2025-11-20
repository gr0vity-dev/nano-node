use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{AccountBalanceResponse, AccountsBalancesResponse, WalletBalancesArgs};
use rsnano_types::Amount;
use std::collections::HashMap;

impl RpcCommandHandler {
    pub(crate) fn wallet_balances(
        &self,
        args: WalletBalancesArgs,
    ) -> anyhow::Result<AccountsBalancesResponse> {
        let threshold = args.threshold.unwrap_or(Amount::ZERO);
        let accounts = self
            .wallet_services
            .wallets
            .get_accounts_of_wallet(&args.wallet)?;
        let mut balances = HashMap::new();
        for account in accounts {
            let balance = self.ledger_queries.account_balance(&account);

            if balance >= threshold {
                let pending = self.ledger_queries.account_receivable(&account);

                let account_balance = AccountBalanceResponse {
                    balance,
                    pending,
                    receivable: pending,
                };
                balances.insert(account, account_balance);
            }
        }
        Ok(AccountsBalancesResponse { balances })
    }
}
