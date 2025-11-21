use crate::command_handler::RpcCommandHandler;
use indexmap::IndexMap;
use rsnano_rpc_messages::{
    RepresentativesArgs, RepresentativesResponse, unwrap_bool_or_false, unwrap_u64_or_max,
};
use rsnano_types::{Account, Amount};

impl RpcCommandHandler {
    pub(crate) fn representatives(&self, args: RepresentativesArgs) -> RepresentativesResponse {
        let count = unwrap_u64_or_max(args.count) as usize;
        let sorting = unwrap_bool_or_false(args.sorting);
        let representatives = if sorting {
            let mut representatives: IndexMap<Account, Amount> = self
                .ledger_queries
                .representative_weights()
                .into_iter()
                .collect();

            representatives.sort_by(|_, v1, _, v2| v2.cmp(v1));
            representatives.truncate(count);
            representatives
        } else {
            self.ledger_queries
                .representative_weights()
                .into_iter()
                .take(count)
                .collect()
        };

        RepresentativesResponse::new(representatives)
    }
}
