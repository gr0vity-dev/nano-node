use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{AccountWeightArgs, WeightDto};

impl RpcCommandHandler {
    pub(crate) fn account_weight(&self, args: AccountWeightArgs) -> WeightDto {
        let weight = self
            .node
            .services()
            .ledger
            .any()
            .weight_exact(args.account.into());
        WeightDto::new(weight)
    }
}
