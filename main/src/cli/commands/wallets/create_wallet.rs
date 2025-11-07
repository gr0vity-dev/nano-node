use crate::cli::{GlobalArgs, build_node};
use anyhow::anyhow;
use clap::Parser;
use rand::Rng;
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
    pub(crate) fn create_wallet(&self, global_args: GlobalArgs) -> anyhow::Result<()> {
        let node = build_node(&global_args)?;
        let wallet_id = WalletId::from_bytes(rand::rng().random());

        node.services.wallets.create(wallet_id);
        println!("{:?}", wallet_id);

        let password = self.password.clone().unwrap_or_default();

        node.services
            .wallets
            .rekey(&wallet_id, &password)
            .map_err(|e| anyhow!("Failed to set wallet password: {:?}", e))?;

        node.services
            .wallets
            .ensure_wallet_is_unlocked(wallet_id, &password);

        if let Some(seed) = &self.seed {
            let key = RawKey::decode_hex(seed).ok_or_else(|| anyhow!("Invalid seed"))?;

            node.services
                .wallets
                .change_seed(wallet_id, &key, 0)
                .map_err(|e| anyhow!("Failed to set wallet seed: {:?}", e))?;
        }

        Ok(())
    }
}
