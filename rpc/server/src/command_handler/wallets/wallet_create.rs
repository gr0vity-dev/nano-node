use rsnano_rpc_messages::{WalletCreateArgs, WalletCreateResponse};
use rsnano_types::WalletId;

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn wallet_create(
        &self,
        args: WalletCreateArgs,
    ) -> anyhow::Result<WalletCreateResponse> {
        let wallet = WalletId::random();
        self.wallet_services.create_wallet(wallet);

        let (last_restored_account, restored_count) = if let Some(seed) = args.seed {
            let (count, last) = self
                .wallet_services
                .restore_wallet_from_seed(wallet, &seed, 0)?;
            (Some(last), Some(count.into()))
        } else {
            (None, None)
        };

        Ok(WalletCreateResponse {
            wallet,
            last_restored_account,
            restored_count,
        })
    }
}
