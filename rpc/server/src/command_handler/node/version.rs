use crate::command_handler::RpcCommandHandler;
use rsnano_node::telemetry::{rsnano_build_info, rsnano_version_string};
use rsnano_rpc_messages::VersionResponse;

impl RpcCommandHandler {
    pub(crate) fn version(&self) -> VersionResponse {
        VersionResponse {
            rpc_version: 1.into(),
            store_version: self.ledger_info.store_version().into(),
            protocol_version: self.node.network_params().network.protocol_version.into(),
            node_vendor: rsnano_version_string(),
            store_vendor: self.ledger_info.store_vendor(),
            network: self
                .node
                .network_params()
                .network
                .current_network
                .as_str()
                .to_owned(),
            network_identifier: self.node.network_params().ledger.genesis_block.hash(),
            build_info: rsnano_build_info(),
        }
    }
}
