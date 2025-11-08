use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{AccountArg, CountResponse};
use rsnano_types::PublicKey;

impl RpcCommandHandler {
    pub(crate) fn delegators_count(&self, args: AccountArg) -> CountResponse {
        let representative: PublicKey = args.account.into();

        let count = self
            .ledger_services
            .ledger
            .any()
            .iter_accounts()
            .filter(|(_, info)| info.representative == representative)
            .count();

        CountResponse::new(count as u64)
    }
}
