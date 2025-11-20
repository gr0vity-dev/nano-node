use anyhow::anyhow;

use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{AccountArg, AccountRepresentativeDto};

impl RpcCommandHandler {
    pub(crate) fn account_representative(
        &self,
        args: AccountArg,
    ) -> anyhow::Result<AccountRepresentativeDto> {
        let account_info = self
            .ledger_queries
            .account_info(&args.account)
            .ok_or_else(|| anyhow!(Self::ACCOUNT_NOT_FOUND))?;
        Ok(AccountRepresentativeDto::new(
            account_info.representative.as_account(),
        ))
    }
}
