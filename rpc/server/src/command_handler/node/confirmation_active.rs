use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{ConfirmationActiveArgs, ConfirmationActiveResponse, unwrap_u64_or_zero};

impl RpcCommandHandler {
    pub(crate) fn confirmation_active(
        &self,
        args: ConfirmationActiveArgs,
    ) -> ConfirmationActiveResponse {
        let announcements = unwrap_u64_or_zero(args.announcements);
        let (elections, confirmed) = self.node.active.confirmation_active_roots(announcements);

        let unconfirmed = elections.len() as u64;
        ConfirmationActiveResponse {
            confirmations: elections,
            unconfirmed: unconfirmed.into(),
            confirmed: confirmed.into(),
        }
    }
}
