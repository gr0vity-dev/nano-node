use anyhow::anyhow;

use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{AccountInfoArgs, AccountInfoResponse, unwrap_bool_or_false};
use rsnano_types::{Amount, Epoch};

impl RpcCommandHandler {
    pub(crate) fn account_info(
        &self,
        args: AccountInfoArgs,
    ) -> anyhow::Result<AccountInfoResponse> {
        let include_confirmed = unwrap_bool_or_false(args.include_confirmed);
        let info = self
            .ledger_queries
            .account_info(&args.account)
            .ok_or_else(|| anyhow!(Self::ACCOUNT_NOT_FOUND))?;

        let conf_info = self
            .ledger_queries
            .confirmation_height_info(&args.account)
            .unwrap_or_default();

        let mut account_info = AccountInfoResponse {
            frontier: info.head,
            open_block: info.open_block,
            representative_block: self.ledger_queries.representative_block_hash(&info.head),
            balance: info.balance,
            modified_timestamp: info.modified.as_u64().into(),
            block_count: info.block_count.into(),
            account_version: (epoch_as_number(info.epoch) as u16).into(),
            confirmed_height: None,
            confirmation_height_frontier: None,
            representative: None,
            weight: None,
            pending: None,
            receivable: None,
            confirmed_balance: None,
            confirmed_pending: None,
            confirmed_receivable: None,
            confirmed_representative: None,
            confirmed_frontier: None,
            confirmation_height: None,
        };

        if include_confirmed {
            let confirmed_balance = if info.block_count != conf_info.height {
                self.ledger_queries
                    .block_balance(&conf_info.frontier)
                    .unwrap_or(Amount::ZERO)
            } else {
                // block_height and confirmed height are the same, so can just reuse balance
                info.balance
            };
            account_info.confirmed_balance = Some(confirmed_balance);
            account_info.confirmed_height = Some(conf_info.height.into());
            account_info.confirmation_height = Some(conf_info.height.into());
            account_info.confirmed_frontier = Some(conf_info.frontier);
        } else {
            // For backwards compatibility purposes
            account_info.confirmation_height = Some(conf_info.height.into());
            account_info.confirmed_height = Some(conf_info.height.into());
            account_info.confirmation_height_frontier = Some(conf_info.frontier);
        }

        if unwrap_bool_or_false(args.representative) {
            account_info.representative = Some(info.representative.into());
            if include_confirmed {
                let confirmed_representative = if conf_info.height > 0 {
                    if let Some(confirmed_frontier_block) =
                        self.ledger_queries.get_block(&conf_info.frontier)
                    {
                        confirmed_frontier_block
                            .representative_field()
                            .unwrap_or_else(|| {
                                let rep_block_hash = self
                                    .ledger_queries
                                    .representative_block_hash(&conf_info.frontier);
                                self.ledger_queries
                                    .get_block(&rep_block_hash)
                                    .unwrap()
                                    .representative_field()
                                    .unwrap()
                            })
                    } else {
                        info.representative
                    }
                } else {
                    info.representative
                };
                account_info.confirmed_representative = Some(confirmed_representative.into());
            }
        }

        if unwrap_bool_or_false(args.weight) {
            account_info.weight = Some(self.ledger_queries.weight_exact(args.account.into()));
        }

        let receivable = unwrap_bool_or_false(args.receivable);
        if receivable {
            let account_receivable = self.ledger_queries.account_receivable(&args.account);
            account_info.pending = Some(account_receivable);
            account_info.receivable = Some(account_receivable);

            if include_confirmed {
                let confirmed_receivable = self
                    .ledger_queries
                    .confirmed_account_receivable(&args.account);
                account_info.confirmed_pending = Some(confirmed_receivable);
                account_info.confirmed_receivable = Some(confirmed_receivable);
            }
        }

        Ok(account_info)
    }
}

fn epoch_as_number(epoch: Epoch) -> u8 {
    match epoch {
        Epoch::Epoch1 => 1,
        Epoch::Epoch2 => 2,
        _ => 0,
    }
}
