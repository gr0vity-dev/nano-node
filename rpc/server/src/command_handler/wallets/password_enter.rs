use rsnano_rpc_messages::{ValidResponse, WalletWithPasswordArgs};
use rsnano_wallet::WalletsError;

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn password_enter(
        &self,
        args: WalletWithPasswordArgs,
    ) -> anyhow::Result<ValidResponse> {
        match self
            .node
            .services
            .wallets
            .enter_password(args.wallet, &args.password)
        {
            Ok(_) => Ok(ValidResponse::new(true)),
            Err(WalletsError::InvalidPassword) => Ok(ValidResponse::new(false)),
            Err(e) => Err(e.into()),
        }
    }
}
