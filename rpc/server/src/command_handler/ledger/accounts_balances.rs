use crate::command_handler::RpcCommandHandler;
use rsnano_ledger::LedgerSet;
use rsnano_rpc_messages::{
    AccountBalanceResponse, AccountsBalancesArgs, AccountsBalancesResponse, unwrap_bool_or_true,
};
use std::collections::HashMap;

impl RpcCommandHandler {
    pub(crate) fn accounts_balances(&self, args: AccountsBalancesArgs) -> AccountsBalancesResponse {
        let only_confirmed = unwrap_bool_or_true(args.include_only_confirmed);
        if only_confirmed {
            let set = self.ledger_services.ledger.confirmed();
            get_account_balances(set, &args)
        } else {
            let set = self.ledger_services.ledger.any();
            get_account_balances(set, &args)
        }
    }
}

fn get_account_balances(
    set: impl LedgerSet,
    args: &AccountsBalancesArgs,
) -> AccountsBalancesResponse {
    let mut balances = HashMap::new();

    for account in &args.accounts {
        let balance = set.account_balance(account);
        let pending = set.account_receivable(account);

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
