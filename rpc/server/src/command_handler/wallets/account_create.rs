use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{AccountCreateArgs, AccountResponse};

impl RpcCommandHandler {
    pub(crate) fn account_create(
        &self,
        args: AccountCreateArgs,
    ) -> anyhow::Result<AccountResponse> {
        let generate_work = args.work.unwrap_or(true.into()).inner();

        let account = match args.index {
            Some(i) => self.node.services.wallets.deterministic_insert_at(
                &args.wallet,
                i.inner(),
                generate_work,
            )?,
            None => self
                .node
                .services
                .wallets
                .deterministic_insert2(&args.wallet, generate_work)?,
        };

        Ok(AccountResponse::new(account.as_account()))
    }
}
