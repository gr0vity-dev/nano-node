use crate::cli::{GlobalArgs, build_node};
use anyhow::anyhow;
use clap::Parser;
use rsnano_types::WalletId;

#[derive(Parser, PartialEq, Debug)]
pub(crate) struct DestroyWalletArgs {
    /// The wallet to be destroyed
    #[arg(long)]
    wallet: String,
    /// Optional password to unlock the wallet
    #[arg(long)]
    password: Option<String>,
}

impl DestroyWalletArgs {
    pub(crate) fn destroy_wallet(&self, global_args: GlobalArgs) -> anyhow::Result<()> {
        let node = build_node(&global_args)?;
        let wallet_id =
            WalletId::decode_hex(&self.wallet).ok_or_else(|| anyhow!("Invalid wallet id"))?;
        let password = self.password.clone().unwrap_or_default();
        node.services
            .wallets
            .ensure_wallet_is_unlocked(wallet_id, &password);
        node.services.wallets.destroy(&wallet_id);
        Ok(())
    }
}
