use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{UnopenedArgs, UnopenedResponse, unwrap_u64_or_max};
use rsnano_types::{Account, Amount, BlockHash, PendingKey};
use std::collections::HashMap;

impl RpcCommandHandler {
    pub(crate) fn unopened(&self, args: UnopenedArgs) -> UnopenedResponse {
        let count = unwrap_u64_or_max(args.count) as usize;
        let threshold = args.threshold.unwrap_or_default();
        let start = args.account.unwrap_or(Account::from(1)); // exclude burn account by default
        let mut accounts: HashMap<Account, Amount> = HashMap::new();

        let mut iterator = self
            .ledger_queries
            .pending_from(PendingKey::new(start, BlockHash::ZERO))
            .into_iter();

        let mut current_account = start;
        let mut current_account_sum = Amount::ZERO;

        let mut current = iterator.next();
        while let Some(cur) = current {
            if accounts.len() >= count {
                break;
            }

            let (key, info) = cur;
            let account = key.receiving_account;

            if self.ledger_queries.account_info(&account).is_some() {
                if account == Account::MAX {
                    break;
                }
                // Skip existing accounts
                iterator = self
                    .ledger_queries
                    .pending_from(PendingKey::new(account.inc().unwrap(), BlockHash::ZERO))
                    .into_iter();
                current = iterator.next();
            } else {
                if account != current_account {
                    if !current_account_sum.is_zero() {
                        if current_account_sum >= threshold {
                            accounts.insert(current_account, current_account_sum);
                        }
                        current_account_sum = Amount::ZERO;
                    }
                    current_account = account;
                }
                current_account_sum += info.amount;
                iterator.next();
                current = iterator.next();
            }
        }

        // last one after iterator reaches end
        if accounts.len() < count
            && !current_account_sum.is_zero()
            && current_account_sum >= threshold
        {
            accounts.insert(current_account, current_account_sum);
        }

        UnopenedResponse::new(accounts)
    }
}
