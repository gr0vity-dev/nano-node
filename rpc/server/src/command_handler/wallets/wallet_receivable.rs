use crate::command_handler::RpcCommandHandler;
use indexmap::IndexMap;
use rsnano_rpc_messages::{
    AccountsReceivableResponse, AccountsReceivableSimple, AccountsReceivableSource,
    AccountsReceivableThreshold, SourceInfo, WalletReceivableArgs,
};
use rsnano_types::BlockHash;

impl RpcCommandHandler {
    pub(crate) fn wallet_receivable(
        &self,
        args: WalletReceivableArgs,
    ) -> anyhow::Result<AccountsReceivableResponse> {
        let count = args.count.unwrap_or(u32::MAX.into()).inner();
        let threshold = args.threshold.unwrap_or_default();
        let source = args.source.unwrap_or_default().inner();
        let min_version = args.min_version.unwrap_or_default().inner();
        let include_only_confirmed = args.include_only_confirmed.unwrap_or(true.into()).inner();

        let accounts = self
            .wallet_services
            .accounts_of_wallet(&args.wallet)?;

        let mut pending_source = IndexMap::new();
        let mut pending_threshold = IndexMap::new();
        let mut pending_default = IndexMap::new();

        for account in accounts {
            let mut block_source = IndexMap::new();
            let mut block_threshold = IndexMap::new();
            let mut block_default = Vec::new();

            for (key, info) in self
                .ledger_queries
                .receivable_upper_bound(account, BlockHash::ZERO)
                .take(count as usize)
            {
                if include_only_confirmed
                    && !self
                        .ledger_queries
                        .confirmed_block_exists(&key.send_block_hash)
                {
                    continue;
                }

                if threshold.is_zero() && !source {
                    block_default.push(key.send_block_hash);
                    continue;
                }

                if source || min_version {
                    block_source.insert(
                        key.send_block_hash,
                        SourceInfo {
                            amount: info.amount,
                            source: source.then_some(info.source),
                            min_version: min_version.then(|| info.epoch.epoch_number().into()),
                        },
                    );
                } else {
                    block_threshold.insert(key.send_block_hash, info.amount);
                }
            }
            if !block_source.is_empty() {
                pending_source.insert(account, block_source);
            } else if !block_threshold.is_empty() {
                pending_threshold.insert(account, block_threshold);
            } else if !block_default.is_empty() {
                pending_default.insert(account, block_default);
            }
        }

        if threshold.is_zero() && !source {
            Ok(AccountsReceivableResponse::Simple(
                AccountsReceivableSimple {
                    blocks: pending_default,
                },
            ))
        } else if source || min_version {
            Ok(AccountsReceivableResponse::Source(
                AccountsReceivableSource {
                    blocks: pending_source,
                },
            ))
        } else {
            Ok(AccountsReceivableResponse::Threshold(
                AccountsReceivableThreshold {
                    blocks: pending_threshold,
                },
            ))
        }
    }
}
