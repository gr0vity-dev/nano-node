use anyhow::anyhow;
use clap::Parser;
use rsnano_node::services::WalletServices;
use rsnano_types::WalletId;

#[derive(Parser, PartialEq, Debug)]
pub(crate) struct DecryptWalletArgs {
    /// The wallet to be decrypted
    #[arg(long)]
    wallet: String,
    /// Optional password to unlock the wallet
    #[arg(long)]
    password: Option<String>,
}

impl DecryptWalletArgs {
    pub(crate) fn decrypt_wallet(&self, wallet_services: &WalletServices) -> anyhow::Result<()> {
        let wallet_id =
            WalletId::decode_hex(&self.wallet).ok_or_else(|| anyhow!("Invalid wallet id"))?;
        let password = self.password.clone().unwrap_or_default();

        wallet_services
            .wallets
            .ensure_wallet_is_unlocked(wallet_id, &password);

        let seed = wallet_services
            .wallets
            .get_seed(wallet_id)
            .map_err(|e| anyhow!("Failed to get wallet seed: {:?}", e))?;

        println!("Seed: {:?}", seed);
        Ok(())
    }
}
