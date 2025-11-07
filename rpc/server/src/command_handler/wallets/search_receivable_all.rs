use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::SuccessResponse;

impl RpcCommandHandler {
    pub(crate) fn search_receivable_all(&self) -> SuccessResponse {
        self.node.services.wallets.search_receivable_all();
        SuccessResponse::new()
    }
}
