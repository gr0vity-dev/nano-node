use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{JsonResponse, WalletRpcMessage};

impl RpcCommandHandler {
    pub(crate) fn wallet_export(&self, args: WalletRpcMessage) -> anyhow::Result<JsonResponse> {
        let json = self.wallet_services.export_wallet_json(args.wallet)?;
        Ok(JsonResponse::new(json))
    }
}
