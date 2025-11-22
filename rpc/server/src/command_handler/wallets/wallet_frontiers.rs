use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{FrontiersResponse, WalletRpcMessage};
use std::collections::HashMap;

impl RpcCommandHandler {
    pub(crate) fn wallet_frontiers(
        &self,
        args: WalletRpcMessage,
    ) -> anyhow::Result<FrontiersResponse> {
        let accounts = self
            .wallet_services
            .accounts_of_wallet(&args.wallet)?;
        let mut frontiers = HashMap::new();

        for account in accounts {
            if let Some(block_hash) = self.ledger_queries.account_head(&account) {
                frontiers.insert(account, block_hash);
            }
        }
        Ok(FrontiersResponse::new(frontiers))
    }
}
