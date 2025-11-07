use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{FrontiersArgs, FrontiersResponse};

impl RpcCommandHandler {
    pub(crate) fn frontiers(&self, args: FrontiersArgs) -> FrontiersResponse {
        let frontiers = self
            .node
            .services()
            .ledger
            .any()
            .iter_account_range(args.account..)
            .map(|(account, info)| (account, info.head))
            .take(args.count.into())
            .collect();

        FrontiersResponse::new(frontiers)
    }
}
