use rsnano_ledger::{AnySet, LedgerSet};
use rsnano_rpc_messages::{HashRpcMessage, StartedResponse};

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn block_confirm(&self, args: HashRpcMessage) -> anyhow::Result<StartedResponse> {
        let any = self.services.ledger.any();
        let block = self.load_block_any(&any, &args.hash)?;
        if !any.confirmed().block_exists(&args.hash) {
            // Start new confirmation for unconfirmed (or not being confirmed) block
            if !self.services.confirming_set.contains(&args.hash) {
                self.services.election_schedulers.manual.push(block);
            }
        }
        Ok(StartedResponse::new(true))
    }
}
