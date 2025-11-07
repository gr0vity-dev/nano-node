use anyhow::anyhow;

use rsnano_ledger::AnySet;
use rsnano_rpc_messages::{
    BlockInfoArgs, BlockInfoResponse, BlockSubTypeDto, unwrap_bool_or_false,
};
use rsnano_types::{BlockType, SavedBlock, UnixTimestamp};

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn block_info(&self, args: BlockInfoArgs) -> anyhow::Result<BlockInfoResponse> {
        let include_linked_account = unwrap_bool_or_false(args.include_linked_account);
        let any = self.node.services.ledger.any();
        let block = any
            .detailed_block(&args.hash)
            .ok_or_else(|| anyhow!(Self::BLOCK_NOT_FOUND))?;

        let linked_account = if include_linked_account {
            match any.linked_account(&block.block) {
                Some(a) => Some(a.encode_account()),
                None => Some("0".to_owned()),
            }
        } else {
            None
        };

        Ok(BlockInfoResponse {
            block_account: block.block.account(),
            amount: block.amount,
            balance: block.block.balance(),
            height: block.block.height().into(),
            local_timestamp: UnixTimestamp::from(block.block.timestamp()).as_u64().into(),
            successor: any.block_successor(&block.block.hash()).unwrap_or_default(),
            confirmed: block.confirmed.into(),
            contents: block.block.json_representation(),
            subtype: Self::subtype_for(&block.block),
            source_account: None,
            receive_hash: None,
            receivable: None,
            linked_account,
        })
    }

    fn subtype_for(block: &SavedBlock) -> Option<BlockSubTypeDto> {
        if block.block_type() == BlockType::State {
            Some(block.subtype().into())
        } else {
            None
        }
    }
}
