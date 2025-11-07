use crate::command_handler::RpcCommandHandler;
use rsnano_ledger::AnySet;
use rsnano_rpc_messages::{FrontiersResponse, WalletRpcMessage};
use std::collections::HashMap;

impl RpcCommandHandler {
    pub(crate) fn wallet_frontiers(
        &self,
        args: WalletRpcMessage,
    ) -> anyhow::Result<FrontiersResponse> {
        let any = self.ledger_services.ledger.any();
        let accounts = self
            .node
            .services()
            .wallets
            .get_accounts_of_wallet(&args.wallet)?;
        let mut frontiers = HashMap::new();

        for account in accounts {
            if let Some(block_hash) = any.account_head(&account) {
                frontiers.insert(account, block_hash);
            }
        }
        Ok(FrontiersResponse::new(frontiers))
    }
}
