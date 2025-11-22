use anyhow::anyhow;

use rsnano_rpc_messages::{HashRpcMessage, StartedResponse};

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn block_confirm(&self, args: HashRpcMessage) -> anyhow::Result<StartedResponse> {
        let block = self
            .ledger_queries
            .get_block(&args.hash)
            .ok_or_else(|| anyhow!(Self::BLOCK_NOT_FOUND))?;
        if !self.ledger_queries.confirmed_block_exists(&args.hash) {
            // Start new confirmation for unconfirmed (or not being confirmed) block
            if !self.ledger_services.confirming_set.contains(&args.hash) {
                self.consensus.push_manual(block);
            }
        }
        Ok(StartedResponse::new(true))
    }
}
