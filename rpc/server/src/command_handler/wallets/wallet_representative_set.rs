use rsnano_rpc_messages::{SetResponse, WalletRepresentativeSetArgs};

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn wallet_representative_set(
        &self,
        args: WalletRepresentativeSetArgs,
    ) -> anyhow::Result<SetResponse> {
        let update_existing = args.update_existing_accounts.unwrap_or_default().inner();
        self.wallet_services
            .wallets
            .set_representative(args.wallet, args.representative.into(), update_existing)
            .wait()?;
        Ok(SetResponse::new(true))
    }
}
