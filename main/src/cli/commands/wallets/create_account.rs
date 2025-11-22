use anyhow::anyhow;
use clap::Parser;
use rsnano_node::services::WalletServices;
use rsnano_types::{Account, WalletId};

#[derive(Parser, PartialEq, Debug)]
pub(crate) struct CreateAccountArgs {
    /// Creates an account in the supplied <wallet>
    #[arg(long)]
    wallet: String,
    /// Optional password to unlock the wallet
    #[arg(long)]
    password: Option<String>,
}

impl CreateAccountArgs {
    pub(crate) fn create_account(&self, wallet_services: &WalletServices) -> anyhow::Result<()> {
        let wallet =
            WalletId::decode_hex(&self.wallet).ok_or_else(|| anyhow!("Invalid wallet id"))?;
        let password = self.password.clone().unwrap_or_default();

        wallet_services.ensure_wallet_is_unlocked(wallet, &password);

        let public_key = wallet_services
            .deterministic_insert(&wallet, false)
            .map_err(|e| anyhow!("Failed to insert wallet: {:?}", e))?;

        println!("Account: {:?}", Account::from(public_key).encode_account());

        Ok(())
    }
}
