use anyhow::anyhow;
use clap::Parser;
use rsnano_node::services::WalletServices;
use rsnano_types::{Account, WalletId};

#[derive(Parser, PartialEq, Debug)]
pub(crate) struct GetWalletRepresentativeArgs {
    /// Gets the representative of the supplied <wallet>
    #[arg(long)]
    wallet: String,
    /// Optional password to unlock the wallet
    #[arg(long)]
    password: Option<String>,
}

impl GetWalletRepresentativeArgs {
    pub(crate) fn get_wallet_representative(
        &self,
        wallet_services: &WalletServices,
    ) -> anyhow::Result<()> {
        let wallet_id =
            WalletId::decode_hex(&self.wallet).ok_or_else(|| anyhow!("Invalid wallet id"))?;
        let password = self.password.clone().unwrap_or_default();

        wallet_services.ensure_wallet_is_unlocked(wallet_id, &password);

        let representative = wallet_services
            .wallet_representative(wallet_id)
            .map_err(|e| anyhow!("Failed to get wallet representative: {:?}", e))?;

        println!(
            "Representative: {:?}",
            Account::from(representative).encode_account()
        );

        Ok(())
    }
}
