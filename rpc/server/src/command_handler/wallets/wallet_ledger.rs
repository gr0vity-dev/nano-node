use std::{collections::HashMap, sync::Arc};

use rsnano_ledger::{AnySet, Ledger, LedgerSet};
use rsnano_rpc_messages::{AccountInfo, WalletLedgerArgs, WalletLedgerResponse};
use rsnano_types::{Account, UnixTimestamp};

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn wallet_ledger(
        &self,
        args: WalletLedgerArgs,
    ) -> anyhow::Result<WalletLedgerResponse> {
        let representative = args.representative.unwrap_or_default().inner();
        let weight = args.weight.unwrap_or_default().inner();
        let receivable = args.receivable.unwrap_or_default().inner();
        let modified_since = args.modified_since.unwrap_or_default().inner();

        let accounts = self
            .wallet_services
            .wallets
            .get_accounts_of_wallet(&args.wallet)?;
        let account_dtos = get_accounts_info(
            self.ledger_services.ledger.clone(),
            accounts,
            representative,
            weight,
            receivable,
            modified_since.into(),
        );
        Ok(WalletLedgerResponse {
            accounts: account_dtos,
        })
    }
}

fn get_accounts_info(
    ledger: Arc<Ledger>,
    accounts: Vec<Account>,
    representative: bool,
    weight: bool,
    receivable: bool,
    modified_since: UnixTimestamp,
) -> HashMap<Account, AccountInfo> {
    let any = ledger.any();
    let mut account_dtos = HashMap::new();

    for account in accounts {
        if let Some(info) = any.get_account(&account)
            && info.modified >= modified_since
        {
            let entry = AccountInfo {
                frontier: info.head,
                open_block: info.open_block,
                representative_block: any.representative_block_hash(&info.head),
                balance: info.balance,
                modified_timestamp: info.modified.as_u64().into(),
                block_count: info.block_count.into(),
                representative: representative.then(|| info.representative.as_account()),
                weight: weight.then(|| any.weight_exact(account.into())),
                receivable: receivable.then(|| any.account_receivable(&account)),
                pending: receivable.then(|| any.account_receivable(&account)),
            };

            account_dtos.insert(account, entry);
        }
    }

    account_dtos
}
