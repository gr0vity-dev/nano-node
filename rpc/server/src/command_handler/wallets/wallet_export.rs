use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{JsonResponse, WalletRpcMessage};

impl RpcCommandHandler {
    pub(crate) fn wallet_export(&self, args: WalletRpcMessage) -> anyhow::Result<JsonResponse> {
        let json = self.wallet_services.wallets.serialize(args.wallet)?;
        Ok(JsonResponse::new(json))
    }
}
