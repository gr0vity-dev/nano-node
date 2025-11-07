use crate::command_handler::RpcCommandHandler;
use anyhow::bail;
use rsnano_rpc_messages::{BlockDto, SendArgs};
use rsnano_types::{BlockDetails, WorkNonce};

impl RpcCommandHandler {
    pub(crate) fn send(&self, args: SendArgs) -> anyhow::Result<BlockDto> {
        let wallet_id = args.wallet;
        let amount = args.amount;
        // Sending 0 amount is invalid with state blocks
        if amount.is_zero() {
            bail!("Invalid amount number");
        }
        let source = args.source;
        let destination = args.destination;
        let work: WorkNonce = args.work.unwrap_or_default();
        if work.is_zero() && !self.wallet_services.work_factory.work_generation_enabled() {
            bail!("Work generation is disabled");
        }

        let any = self.services.ledger.any();
        let info = self.load_account(&any, &source)?;
        let balance = info.balance;

        if !work.is_zero() {
            let details = BlockDetails::new(info.epoch, true, false, false);
            if self
                .node
                .network_params
                .work
                .difficulty(&info.head.into(), work)
                < self.node.network_params.work.threshold(&details)
            {
                bail!("Invalid work")
            }
        }

        let generate_work = work.is_zero(); // Disable work generation if "work" option is provided
        let send_id = args.id;

        let block = self
            .wallet_services
            .wallets
            .send(
                wallet_id,
                source,
                destination,
                amount,
                work,
                generate_work,
                send_id,
            )
            .wait();

        match block {
            Ok(block) => Ok(BlockDto::new(block.hash())),
            Err(_) => {
                if balance >= amount {
                    bail!("Error generating block")
                } else {
                    bail!("Insufficient balance")
                }
            }
        }
    }
}
