use anyhow::anyhow;
use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{
    AccountArg, AccountBalanceArgs, AccountBalanceResponse, AccountBlockCountResponse,
    unwrap_bool_or_true,
};

impl RpcCommandHandler {
    pub(crate) fn account_balance(&self, args: AccountBalanceArgs) -> AccountBalanceResponse {
        let only_confirmed = unwrap_bool_or_true(args.include_only_confirmed);
        let (balance, receivable) = if only_confirmed {
            self.ledger_account_balances
                .confirmed_balance_and_receivable(&args.account)
        } else {
            self.ledger_account_balances
                .any_balance_and_receivable(&args.account)
        };

        AccountBalanceResponse {
            balance,
            pending: receivable,
            receivable,
        }
    }

    pub(crate) fn account_block_count(
        &self,
        args: AccountArg,
    ) -> anyhow::Result<AccountBlockCountResponse> {
        let block_count = self
            .ledger_account_balances
            .account_block_count(&args.account)
            .ok_or_else(|| anyhow!(Self::ACCOUNT_NOT_FOUND))?;
        Ok(AccountBlockCountResponse::new(block_count))
    }
}
