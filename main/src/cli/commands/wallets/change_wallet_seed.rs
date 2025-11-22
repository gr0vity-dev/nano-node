use anyhow::anyhow;
use clap::Parser;
use rsnano_node::services::WalletServices;
use rsnano_types::{RawKey, WalletId};

#[derive(Parser, PartialEq, Debug)]
pub(crate) struct ChangeWalletSeedArgs {
    /// Changes the seed of the supplied wallet
    #[arg(long)]
    wallet: String,
    /// The new <seed> of the wallet
    #[arg(long)]
    seed: String,
    /// Optional <password> to unlock the wallet
    #[arg(long)]
    password: Option<String>,
}

impl ChangeWalletSeedArgs {
    pub(crate) fn change_wallet_seed(
        &self,
        wallet_services: &WalletServices,
    ) -> anyhow::Result<()> {
        let wallet_id =
            WalletId::decode_hex(&self.wallet).ok_or_else(|| anyhow!("Invalid wallet id"))?;
        let seed = RawKey::decode_hex(&self.seed).ok_or_else(|| anyhow!("Invalid seed"))?;
        let password = self.password.clone().unwrap_or_default();

        wallet_services.ensure_wallet_is_unlocked(wallet_id, &password);

        wallet_services
            .change_wallet_seed(wallet_id, &seed, 0)
            .map_err(|e| anyhow!("Failed to change wallet seed: {:?}", e))?;

        Ok(())
    }
}
