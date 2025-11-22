use std::cmp::max;

use anyhow::{anyhow, bail};
use rsnano_rpc_messages::{BlockDto, ReceiveArgs};
use rsnano_types::{Amount, BlockDetails, PendingKey, Root, WorkNonce};
use rsnano_wallet::WalletsError;

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub fn receive(&self, args: ReceiveArgs) -> anyhow::Result<BlockDto> {
        if !self.ledger_queries.block_exists(&args.block) {
            bail!(Self::BLOCK_NOT_FOUND);
        }

        let Some(pending_info) = self
            .ledger_queries
            .get_pending(&PendingKey::new(args.account, args.block))
        else {
            bail!("Block is not receivable");
        };

        let work: WorkNonce = if let Some(work) = args.work {
            let (head, epoch) = if let Some(info) = self.ledger_queries.account_info(&args.account)
            {
                // When receiving, epoch version is the higher between the previous and the source blocks
                let epoch = max(info.epoch, pending_info.epoch);
                (Root::from(info.head), epoch)
            } else {
                (Root::from(args.account), pending_info.epoch)
            };
            let details = BlockDetails::new(epoch, false, true, false);
            if self
                .node
                .network_params()
                .work
                .difficulty(&head, work)
                < self.node.network_params().work.threshold(&details)
            {
                bail!("Invalid work")
            }
            work
        } else if self.wallet_services.work_generation_enabled() {
            0.into()
        } else {
            bail!("Work generation is disabled");
        };

        // Representative is only used by receive_action when opening accounts
        // Set a wallet default representative for new accounts
        let representative = self
            .wallet_services
            .wallet_representative(args.wallet)?;

        // Disable work generation if "work" option is provided
        let generate_work = work.is_zero();

        let block = self
            .wallet_services
            .receive(
                args.wallet,
                args.block,
                representative,
                Amount::MAX,
                args.account,
                work,
                generate_work,
            )
            .wait()
            .map_err(|e| match e {
                WalletsError::WalletNotFound => anyhow!("wallet not found"),
                _ => anyhow!("Error generating block"),
            })?;

        Ok(BlockDto::new(block.hash()))
    }
}
