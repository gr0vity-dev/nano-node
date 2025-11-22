use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{WalletWithAccountArgs, WorkResponse};

impl RpcCommandHandler {
    pub(crate) fn work_get(&self, args: WalletWithAccountArgs) -> anyhow::Result<WorkResponse> {
        let work = self
            .wallet_services
            .work_get(&args.wallet, &args.account.into())?;
        Ok(WorkResponse::new(work))
    }
}
