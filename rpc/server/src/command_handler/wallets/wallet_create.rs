use rsnano_rpc_messages::{WalletCreateArgs, WalletCreateResponse};
use rsnano_types::WalletId;

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn wallet_create(
        &self,
        args: WalletCreateArgs,
    ) -> anyhow::Result<WalletCreateResponse> {
        let wallet = WalletId::random();
        self.node.services.wallets.create(wallet);

        let last_restored_account;
        let restored_count;
        if let Some(seed) = args.seed {
            let (count, last) = self.node.services.wallets.change_seed(wallet, &seed, 0)?;
            last_restored_account = Some(last);
            restored_count = Some(count.into());
        } else {
            last_restored_account = None;
            restored_count = None;
        }

        Ok(WalletCreateResponse {
            wallet,
            last_restored_account,
            restored_count,
        })
    }
}
