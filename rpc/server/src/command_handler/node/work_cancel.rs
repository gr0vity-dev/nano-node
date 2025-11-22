use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{HashRpcMessage, SuccessResponse};

impl RpcCommandHandler {
    pub(crate) fn work_cancel(&self, args: HashRpcMessage) -> SuccessResponse {
        self.bootstrap.cancel_work(args.hash.into());
        SuccessResponse::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::command_handler::{test_rpc_command_requires_control, test_rpc_command_with_node};
    use rsnano_node::Node;
    use rsnano_rpc_messages::{HashRpcMessage, RpcCommand, SuccessResponse};
    use rsnano_types::Root;
    use std::sync::Arc;

    #[test]
    fn without_rpc_control_enabled() {
        test_rpc_command_requires_control(RpcCommand::WorkCancel(HashRpcMessage {
            hash: 1.into(),
        }));
    }

    #[test]
    fn handle_work_cancel_command() {
        let node = Arc::new(Node::new_null());
        let cancel_tracker = node
            .bootstrap_subsystem()
            .test_handles()
            .work_factory
            .track_cancellations();
        let root = Root::from(42);

        let _result: SuccessResponse = test_rpc_command_with_node(
            RpcCommand::WorkCancel(HashRpcMessage { hash: root.into() }),
            node,
        );

        assert_eq!(cancel_tracker.output(), [root]);
    }
}
