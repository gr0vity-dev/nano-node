use crate::command_handler::RpcCommandHandler;
use rsnano_ledger::{AnySet, ConfirmedSet, LedgerSet};
use rsnano_rpc_messages::{WalletInfoResponse, WalletRpcMessage};
use rsnano_store_lmdb::KeyType;
use rsnano_types::Amount;

impl RpcCommandHandler {
    pub(crate) fn wallet_info(&self, args: WalletRpcMessage) -> anyhow::Result<WalletInfoResponse> {
        let accounts = self.services.wallets.decrypt(args.wallet)?;
        let mut balance = Amount::ZERO;
        let mut receivable = Amount::ZERO;
        let mut accounts_count = 0u64;
        let mut block_count = 0u64;
        let mut cemented_count = 0u64;
        let mut deterministic_count = 0u64;
        let mut adhoc_count = 0u64;
        let any = self.services.ledger.any();

        for (account, _priv_key) in accounts {
            let account = account.into();
            if let Some(account_info) = any.get_account(&account) {
                block_count += account_info.block_count;
                balance += account_info.balance;
            }

            if let Some(confirmation_info) = any.confirmed().get_conf_info(&account) {
                cemented_count += confirmation_info.height;
            }

            receivable += any.account_receivable(&account);

            match self
                .node
                .services()
                .wallets
                .key_type(args.wallet, &account.into())
            {
                KeyType::Deterministic => deterministic_count += 1,
                KeyType::Adhoc => adhoc_count += 1,
                _ => {}
            }

            accounts_count += 1;
        }

        let deterministic_index = self
            .node
            .services()
            .wallets
            .deterministic_index_get(&args.wallet)
            .unwrap();

        Ok(WalletInfoResponse {
            balance,
            receivable,
            pending: receivable,
            accounts_count: accounts_count.into(),
            adhoc_count: adhoc_count.into(),
            deterministic_count: deterministic_count.into(),
            deterministic_index: deterministic_index.into(),
            accounts_block_count: block_count.into(),
            accounts_cemented_block_count: cemented_count.into(),
        })
    }
}
