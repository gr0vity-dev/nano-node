use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::UptimeResponse;

impl RpcCommandHandler {
    pub(crate) fn uptime(&self) -> UptimeResponse {
        UptimeResponse::new(self.telemetry_services.uptime().as_secs())
    }
}
