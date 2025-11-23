use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{AccountsWithWorkResponse, WalletRpcMessage};
use std::collections::HashMap;

impl RpcCommandHandler {
    pub(crate) fn wallet_work_get(
        &self,
        args: WalletRpcMessage,
    ) -> anyhow::Result<AccountsWithWorkResponse> {
        let accounts = self.wallet_services.accounts_of_wallet(&args.wallet)?;
        let mut works = HashMap::new();

        for account in accounts {
            let work = self
                .wallet_services
                .work_get(&args.wallet, &account.into())?;
            works.insert(account, work);
        }

        Ok(AccountsWithWorkResponse::new(works))
    }
}
