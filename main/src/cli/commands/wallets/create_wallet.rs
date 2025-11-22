use anyhow::anyhow;
use clap::Parser;
use rand::Rng;
use rsnano_node::services::WalletServices;
use rsnano_types::{RawKey, WalletId};

#[derive(Parser, PartialEq, Debug)]
pub(crate) struct CreateWalletArgs {
    /// Optional seed of the new wallet
    #[arg(long)]
    seed: Option<String>,
    /// Optional password of the new wallet
    #[arg(long)]
    password: Option<String>,
}

impl CreateWalletArgs {
    pub(crate) fn create_wallet(&self, wallet_services: &WalletServices) -> anyhow::Result<()> {
        let wallet_id = WalletId::from_bytes(rand::rng().random());

        wallet_services.create_wallet(wallet_id);
        println!("{:?}", wallet_id);

        let password = self.password.clone().unwrap_or_default();

        wallet_services
            .rekey_wallet(&wallet_id, &password)
            .map_err(|e| anyhow!("Failed to set wallet password: {:?}", e))?;

        wallet_services.ensure_wallet_is_unlocked(wallet_id, &password);

        if let Some(seed) = &self.seed {
            let key = RawKey::decode_hex(seed).ok_or_else(|| anyhow!("Invalid seed"))?;

            wallet_services
                .change_wallet_seed(wallet_id, &key, 0)
                .map_err(|e| anyhow!("Failed to set wallet seed: {:?}", e))?;
        }

        Ok(())
    }
}
