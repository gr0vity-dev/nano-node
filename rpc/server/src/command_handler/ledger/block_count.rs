use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::BlockCountResponse;

impl RpcCommandHandler {
    pub(crate) fn block_count(&self) -> BlockCountResponse {
        let count = self.ledger_counts.block_count();
        let unchecked = self.node.unchecked.lock().unwrap().len() as u64;
        let cemented = self.ledger_counts.confirmed_count();
        BlockCountResponse {
            count: count.into(),
            unchecked: unchecked.into(),
            cemented: cemented.into(),
            full: None,
            pruned: None,
        }
    }
}
