use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{AccountResponse, HashRpcMessage};

impl RpcCommandHandler {
    pub(crate) fn block_account(&self, args: HashRpcMessage) -> anyhow::Result<AccountResponse> {
        let any = self.ledger_services.ledger.any();
        let block = self.load_block_any(&any, &args.hash)?;
        Ok(AccountResponse::new(block.account()))
    }
}
