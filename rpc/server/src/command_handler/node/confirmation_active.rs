use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{ConfirmationActiveArgs, ConfirmationActiveResponse};

impl RpcCommandHandler {
    pub(crate) fn confirmation_active(
        &self,
        _args: ConfirmationActiveArgs,
    ) -> ConfirmationActiveResponse {
        // announcements arg isnt' supported yet!
        let mut confirmed = 0;
        let mut elections = Vec::new();

        for election in self.node.aec.election_snapshots() {
            if !election.is_confirmed() {
                elections.push(election.qualified_root().clone());
            } else {
                confirmed += 1;
            }
        }

        let unconfirmed = elections.len() as u64;
        ConfirmationActiveResponse {
            confirmations: elections,
            unconfirmed: unconfirmed.into(),
            confirmed: confirmed.into(),
        }
    }
}
