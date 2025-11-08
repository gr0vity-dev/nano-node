use crate::command_handler::RpcCommandHandler;
use anyhow::anyhow;
use rsnano_ledger::AnySet;
use rsnano_rpc_messages::{BlocksResponse, HashesArgs};
use rsnano_types::{BlockHash, JsonBlock};
use std::collections::HashMap;

impl RpcCommandHandler {
    pub(crate) fn blocks(&self, args: HashesArgs) -> anyhow::Result<BlocksResponse> {
        let mut blocks: HashMap<BlockHash, JsonBlock> = HashMap::new();
        for hash in args.hashes {
            let block = self
                .ledger_services
                .ledger
                .any()
                .get_block(&hash)
                .ok_or_else(|| anyhow!(Self::BLOCK_NOT_FOUND))?;
            blocks.insert(hash, block.json_representation());
        }
        Ok(BlocksResponse::new(blocks))
    }
}
