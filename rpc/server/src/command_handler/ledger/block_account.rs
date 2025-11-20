use anyhow::anyhow;

use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{AccountResponse, HashRpcMessage};

impl RpcCommandHandler {
    pub(crate) fn block_account(&self, args: HashRpcMessage) -> anyhow::Result<AccountResponse> {
        let block = self
            .ledger_queries
            .get_block(&args.hash)
            .ok_or_else(|| anyhow!(Self::BLOCK_NOT_FOUND))?;
        Ok(AccountResponse::new(block.account()))
    }
}
