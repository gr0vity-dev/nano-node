use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::CountResponse;

impl RpcCommandHandler {
    pub(crate) fn frontier_count(&self) -> CountResponse {
        CountResponse::new(self.ledger_account_count.account_count())
    }
}
