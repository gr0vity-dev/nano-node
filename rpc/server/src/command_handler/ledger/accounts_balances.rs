use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{
    AccountBalanceResponse, AccountsBalancesArgs, AccountsBalancesResponse, unwrap_bool_or_true,
};
use std::collections::HashMap;

impl RpcCommandHandler {
    pub(crate) fn accounts_balances(&self, args: AccountsBalancesArgs) -> AccountsBalancesResponse {
        let only_confirmed = unwrap_bool_or_true(args.include_only_confirmed);
        get_account_balances(&args, &self, only_confirmed)
    }
}

fn get_account_balances(
    args: &AccountsBalancesArgs,
    handler: &RpcCommandHandler,
    only_confirmed: bool,
) -> AccountsBalancesResponse {
    let mut balances = HashMap::new();

    for account in &args.accounts {
        let (balance, pending) = if only_confirmed {
            handler
                .ledger_account_balances
                .confirmed_balance_and_receivable(account)
        } else {
            handler
                .ledger_account_balances
                .any_balance_and_receivable(account)
        };

        balances.insert(
            *account,
            AccountBalanceResponse {
                balance,
                pending,
                receivable: pending,
            },
        );
    }

    AccountsBalancesResponse { balances }
}
