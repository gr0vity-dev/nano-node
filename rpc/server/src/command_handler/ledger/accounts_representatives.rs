use crate::command_handler::RpcCommandHandler;
use rsnano_ledger::LedgerSet;
use rsnano_rpc_messages::{AccountsRepresentativesResponse, AccountsRpcMessage};
use rsnano_types::Account;
use std::collections::HashMap;

impl RpcCommandHandler {
    pub(crate) fn accounts_representatives(
        &self,
        args: AccountsRpcMessage,
    ) -> AccountsRepresentativesResponse {
        let any = self.ledger_services.ledger.any();
        let mut representatives: HashMap<Account, Account> = HashMap::new();
        let mut errors: HashMap<Account, String> = HashMap::new();

        for account in args.accounts {
            match any.get_account(&account) {
                Some(account_info) => {
                    representatives.insert(account, account_info.representative.as_account());
                }
                None => {
                    errors.insert(account, "Account not found".to_string());
                }
            }
        }

        AccountsRepresentativesResponse {
            representatives: if representatives.is_empty() {
                None
            } else {
                Some(representatives)
            },
            errors: if errors.is_empty() {
                None
            } else {
                Some(errors)
            },
        }
    }
}
