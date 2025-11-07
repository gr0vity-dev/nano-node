use anyhow::anyhow;
use clap::Parser;
use rsnano_node::services::WalletServices;
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
    pub(crate) fn destroy_wallet(&self, wallet_services: &WalletServices) -> anyhow::Result<()> {
        let wallet_id =
            WalletId::decode_hex(&self.wallet).ok_or_else(|| anyhow!("Invalid wallet id"))?;
        let password = self.password.clone().unwrap_or_default();

        wallet_services
            .wallets
            .ensure_wallet_is_unlocked(wallet_id, &password);
        wallet_services.wallets.destroy(&wallet_id);
        Ok(())
    }
}
