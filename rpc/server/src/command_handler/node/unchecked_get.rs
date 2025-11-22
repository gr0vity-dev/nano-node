use anyhow::anyhow;

use rsnano_rpc_messages::{HashRpcMessage, UncheckedGetResponse};

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn unchecked_get(
        &self,
        args: HashRpcMessage,
    ) -> anyhow::Result<UncheckedGetResponse> {
        self.node
            .unchecked()
            .find(&args.hash)
            .map(|block| UncheckedGetResponse {
                modified_timestamp: 0.into(), // not supported in RsNano
                contents: block.json_representation(),
            })
            .ok_or_else(|| anyhow!(Self::BLOCK_NOT_FOUND))
    }
}
