use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{
    BlockHashesResponse, ChainArgs, unwrap_bool_or_false, unwrap_u64_or_zero,
};
use rsnano_types::BlockHash;

impl RpcCommandHandler {
    pub(crate) fn chain(&self, args: ChainArgs, successors: bool) -> BlockHashesResponse {
        let successors = successors != unwrap_bool_or_false(args.reverse);
        let mut hash = args.block;
        let count: u64 = args.count.into();
        let mut offset = unwrap_u64_or_zero(args.offset);
        let mut blocks = Vec::new();

        while !hash.is_zero() && blocks.len() < count as usize {
            if let Some(block) = self.ledger_queries.get_block(&hash) {
                if offset > 0 {
                    offset -= 1;
                } else {
                    blocks.push(hash);
                }

                hash = if successors {
                    self
                        .ledger_queries
                        .block_successor(&hash)
                        .unwrap_or(BlockHash::ZERO)
                } else {
                    block.previous()
                };
            } else {
                hash = BlockHash::ZERO;
            }
        }

        BlockHashesResponse::new(blocks)
    }
}
