use crate::command_handler::RpcCommandHandler;
use rsnano_ledger::LedgerSet;
use rsnano_rpc_messages::{
    AccountArg, AccountBalanceArgs, AccountBalanceResponse, AccountBlockCountResponse,
    unwrap_bool_or_true,
};

impl RpcCommandHandler {
    pub(crate) fn account_balance(&self, args: AccountBalanceArgs) -> AccountBalanceResponse {
        let only_confirmed = unwrap_bool_or_true(args.include_only_confirmed);
        if only_confirmed {
            let set = self.node.services.ledger.confirmed();
            get_account_balance(set, &args)
        } else {
            let set = self.node.services.ledger.any();
            get_account_balance(set, &args)
        }
    }

    pub(crate) fn account_block_count(
        &self,
        args: AccountArg,
    ) -> anyhow::Result<AccountBlockCountResponse> {
        let any = self.node.services.ledger.any();
        let account = self.load_account(&any, &args.account)?;
        Ok(AccountBlockCountResponse::new(account.block_count))
    }
}

fn get_account_balance(set: impl LedgerSet, args: &AccountBalanceArgs) -> AccountBalanceResponse {
    let balance = set.account_balance(&args.account);
    let receivable = set.account_receivable(&args.account);

    AccountBalanceResponse {
        balance,
        pending: receivable,
        receivable,
    }
}
