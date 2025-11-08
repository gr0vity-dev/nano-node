use rsnano_rpc_messages::{StartedResponse, WalletRpcMessage};
use rsnano_wallet::WalletsError;

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn search_receivable(
        &self,
        args: WalletRpcMessage,
    ) -> anyhow::Result<StartedResponse> {
        match self
            .wallet_services
            .wallets
            .search_receivable(&args.wallet)
            .wait()
        {
            Ok(_) => Ok(StartedResponse::new(true)),
            Err(WalletsError::WalletLocked) => Ok(StartedResponse::new(false)),
            Err(e) => Err(e.into()),
        }
    }
}
