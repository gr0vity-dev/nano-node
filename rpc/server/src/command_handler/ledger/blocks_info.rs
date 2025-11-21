use std::collections::HashMap;

use anyhow::bail;

use rsnano_rpc_messages::{
    BlockInfoResponse, BlocksInfoArgs, BlocksInfoResponse, unwrap_bool_or_false,
};
use rsnano_types::{BlockHash, BlockType, PendingKey, UnixTimestamp};

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn blocks_info(&self, args: BlocksInfoArgs) -> anyhow::Result<BlocksInfoResponse> {
        let receivable = unwrap_bool_or_false(args.receivable);
        let receive_hash = unwrap_bool_or_false(args.receive_hash);
        let source = unwrap_bool_or_false(args.source);
        let include_not_found = unwrap_bool_or_false(args.include_not_found);
        let include_linked_account = unwrap_bool_or_false(args.include_linked_account);

        let mut blocks: HashMap<BlockHash, BlockInfoResponse> = HashMap::new();
        let mut blocks_not_found = Vec::new();

        for hash in args.hashes {
            if let Some(block) = self.ledger_queries.get_block(&hash) {
                let block_account = block.account();
                let amount = self.ledger_queries.block_amount(&hash);
                let balance = self.ledger_queries.block_balance(&hash).unwrap();
                let height = block.height();
                let local_timestamp = block.timestamp();
                let successor = self
                    .ledger_queries
                    .block_successor(&block.hash())
                    .unwrap_or_default();
                let confirmed = self.ledger_queries.confirmed_block_exists(&hash);
                let contents = block.json_representation();

                let subtype = if block.block_type() == BlockType::State {
                    Some(block.subtype().into())
                } else {
                    None
                };

                let linked_account = if include_linked_account {
                    match self.ledger_queries.linked_account(&block) {
                        Some(a) => Some(a.encode_account()),
                        None => Some("0".to_owned()),
                    }
                } else {
                    None
                };

                let mut block_info = BlockInfoResponse {
                    block_account,
                    amount,
                    balance,
                    height: height.into(),
                    local_timestamp: UnixTimestamp::from(local_timestamp).as_u64().into(),
                    successor,
                    confirmed: confirmed.into(),
                    contents,
                    subtype,
                    receivable: None,
                    receive_hash: None,
                    source_account: None,
                    linked_account,
                };

                if receivable || receive_hash {
                    if !block.is_send() {
                        if receivable {
                            block_info.receivable = Some(0.into());
                        }
                        if receive_hash {
                            block_info.receive_hash = Some(BlockHash::ZERO);
                        }
                    } else if self
                        .ledger_queries
                        .get_pending(&PendingKey::new(block.destination_or_link(), hash))
                        .is_some()
                    {
                        if receivable {
                            block_info.receivable = Some(1.into())
                        }
                        if receive_hash {
                            block_info.receive_hash = Some(BlockHash::ZERO);
                        }
                    } else {
                        if receivable {
                            block_info.receivable = Some(0.into());
                        }
                        if receive_hash {
                            let receive_block =
                                self.ledger_queries.find_receive_block_by_send_hash(
                                    &block.destination_or_link(),
                                    &hash,
                                );

                            block_info.receive_hash = Some(match receive_block {
                                Some(b) => b.hash(),
                                None => BlockHash::ZERO,
                            });
                        }
                    }
                }

                if source {
                    if !block.is_receive()
                        || !self.ledger_queries.block_exists(&block.source_or_link())
                    {
                        block_info.source_account = Some("0".to_string());
                    } else {
                        let block_a = self
                            .ledger_queries
                            .get_block(&block.source_or_link())
                            .unwrap();
                        block_info.source_account = Some(block_a.account().encode_account());
                    }
                }

                blocks.insert(hash, block_info);
            } else if include_not_found {
                blocks_not_found.push(hash);
            } else {
                bail!(Self::BLOCK_NOT_FOUND);
            }
        }

        Ok(BlocksInfoResponse {
            blocks,
            blocks_not_found: if include_not_found {
                Some(blocks_not_found)
            } else {
                None
            },
        })
    }
}
