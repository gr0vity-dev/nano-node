use std::collections::HashMap;

use rsnano_ledger::{AnySet, LedgerSet};
use rsnano_rpc_messages::{
    LedgerAccountInfo, LedgerArgs, LedgerResponse, unwrap_bool_or_false, unwrap_u64_or_max,
    unwrap_u64_or_zero,
};
use rsnano_types::{Account, Amount, UnixTimestamp};

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn ledger(&self, args: LedgerArgs) -> LedgerResponse {
        let count = unwrap_u64_or_max(args.count);
        let threshold = args.threshold.unwrap_or(Amount::ZERO);
        let start = args.account.unwrap_or_default();
        let modified_since: UnixTimestamp = unwrap_u64_or_zero(args.modified_since).into();
        let sorting = unwrap_bool_or_false(args.sorting);
        let representative = unwrap_bool_or_false(args.representative);
        let weight = unwrap_bool_or_false(args.weight);
        let receivable = unwrap_bool_or_false(args.receivable);

        let mut accounts: HashMap<Account, LedgerAccountInfo> = HashMap::new();
        let any = self.services.ledger.any();

        if !sorting {
            // Simple
            for (account, info) in any.iter_account_range(start..) {
                if info.modified >= modified_since && (receivable || info.balance >= threshold) {
                    let receivable = if receivable {
                        let account_receivable = any.account_receivable(&account);
                        if info.balance + account_receivable < threshold {
                            continue;
                        }
                        Some(account_receivable)
                    } else {
                        None
                    };

                    let entry = LedgerAccountInfo {
                        frontier: info.head,
                        open_block: info.open_block,
                        representative_block: any.representative_block_hash(&info.head),
                        balance: info.balance,
                        modified_timestamp: info.modified.as_u64().into(),
                        block_count: info.block_count.into(),
                        representative: representative.then(|| info.representative.into()),
                        weight: weight.then(|| any.weight_exact(account.into())),
                        pending: receivable,
                        receivable,
                    };
                    accounts.insert(account, entry);
                    if accounts.len() >= count as usize {
                        break;
                    }
                }
            }
        } else {
            // Sorting
            let mut ledger: Vec<(Amount, Account)> = Vec::new();
            for (account, info) in any.iter_account_range(start..) {
                if info.modified >= modified_since {
                    ledger.push((info.balance, account));
                }
            }

            ledger.sort_by(|a, b| b.cmp(a));

            for (_, account) in ledger {
                if let Some(info) = any.get_account(&account)
                    && (receivable || info.balance >= threshold)
                {
                    let pending = if receivable {
                        let account_receivable = any.account_receivable(&account);
                        if info.balance + account_receivable < threshold {
                            continue;
                        }
                        Some(account_receivable)
                    } else {
                        None
                    };

                    let entry = LedgerAccountInfo {
                        frontier: info.head,
                        open_block: info.open_block,
                        representative_block: any.representative_block_hash(&info.head),
                        balance: info.balance,
                        modified_timestamp: info.modified.as_u64().into(),
                        block_count: info.block_count.into(),
                        representative: representative.then(|| info.representative.into()),
                        weight: weight.then(|| any.weight_exact(account.into())),
                        pending,
                        receivable: pending,
                    };
                    accounts.insert(account, entry);
                    if accounts.len() >= count as usize {
                        break;
                    }
                }
            }
        }

        LedgerResponse { accounts }
    }
}
