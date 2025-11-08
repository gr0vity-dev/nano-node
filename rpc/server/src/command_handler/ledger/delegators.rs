use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{DelegatorsArgs, DelegatorsResponse, unwrap_u64_or};
use rsnano_types::{Account, Amount, PublicKey};

impl RpcCommandHandler {
    pub(crate) fn delegators(&self, args: DelegatorsArgs) -> DelegatorsResponse {
        let representative: PublicKey = args.account.into();
        let count = unwrap_u64_or(args.count, 1024) as usize;
        let threshold = args.threshold.unwrap_or(Amount::ZERO);
        let start_account = args.start.unwrap_or(Account::ZERO).inc_or_max();

        let delegators = self
            .ledger_services
            .ledger
            .any()
            .iter_account_range(start_account..)
            .filter_map(|(account, info)| {
                if info.representative == representative && info.balance >= threshold {
                    Some((account, info.balance))
                } else {
                    None
                }
            })
            .take(count)
            .collect();

        DelegatorsResponse::new(delegators)
    }
}
