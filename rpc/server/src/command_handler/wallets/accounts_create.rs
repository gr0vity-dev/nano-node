use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{AccountsCreateArgs, AccountsRpcMessage, unwrap_bool_or_false};
use rsnano_types::Account;

impl RpcCommandHandler {
    pub(crate) fn accounts_create(
        &self,
        args: AccountsCreateArgs,
    ) -> anyhow::Result<AccountsRpcMessage> {
        let generate_work = unwrap_bool_or_false(args.work);
        let count: usize = args.count.into();
        let wallet = &args.wallet;

        let accounts: Result<Vec<Account>, _> = (0..count)
            .map(|_| {
                self.wallet_services.wallets
                    .deterministic_insert2(wallet, generate_work)
                    .map(Account::from)
            })
            .collect();

        let accounts = accounts?;
        Ok(AccountsRpcMessage::new(accounts))
    }
}
