use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::CountResponse;

impl RpcCommandHandler {
    pub(crate) fn frontier_count(&self) -> CountResponse {
        CountResponse::new(self.services.ledger.account_count())
    }
}
