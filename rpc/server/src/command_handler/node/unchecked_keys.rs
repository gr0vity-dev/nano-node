use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{
    UncheckedKeyDto, UncheckedKeysArgs, UncheckedKeysResponse, unwrap_u64_or_max,
};

impl RpcCommandHandler {
    pub(crate) fn unchecked_keys(&self, args: UncheckedKeysArgs) -> UncheckedKeysResponse {
        let count = unwrap_u64_or_max(args.count) as usize;

        let response: Vec<_> = self
            .node
            .unchecked()
            .blocks_starting_at(args.key.into(), count)
            .into_iter()
            .map(|(dependency, block)| {
                UncheckedKeyDto {
                    key: dependency,
                    hash: block.hash(),
                    modified_timestamp: 0.into(), // not supported in RsNano
                    contents: block.json_representation(),
                }
            })
            .collect();

        UncheckedKeysResponse::new(response)
    }
}
