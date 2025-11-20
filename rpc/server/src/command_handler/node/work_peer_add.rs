use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{AddressWithPortArgs, SuccessResponse};
use rsnano_types::Peer;

impl RpcCommandHandler {
    pub(crate) fn work_peer_add(&self, args: AddressWithPortArgs) -> SuccessResponse {
        self.bootstrap
            .work_factory()
            .add_peer(Peer::new(args.address, args.port.into()));
        SuccessResponse::new()
    }
}
