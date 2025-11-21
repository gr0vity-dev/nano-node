use std::collections::HashMap;

use rsnano_rpc_messages::{CountArgs, UncheckedResponse, unwrap_u64_or_max};

use crate::command_handler::RpcCommandHandler;
use rsnano_types::{BlockHash, JsonBlock};

impl RpcCommandHandler {
    pub(crate) fn unchecked(&self, args: CountArgs) -> UncheckedResponse {
        let count = unwrap_u64_or_max(args.count) as usize;

        let blocks: HashMap<BlockHash, JsonBlock> = self
            .node
            .unchecked()
            .lock()
            .unwrap()
            .iter()
            .map(|(_, block)| (block.hash(), block.json_representation()))
            .take(count)
            .collect();

        UncheckedResponse::new(blocks)
    }
}
