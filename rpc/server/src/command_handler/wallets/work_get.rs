use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{WalletWithAccountArgs, WorkResponse};

impl RpcCommandHandler {
    pub(crate) fn work_get(&self, args: WalletWithAccountArgs) -> anyhow::Result<WorkResponse> {
        let work = self
            .node
            .services()
            .wallets
            .work_get2(&args.wallet, &args.account.into())?;
        Ok(WorkResponse::new(work))
    }
}
