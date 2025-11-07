use crate::cli::{GlobalArgs, build_node};
use anyhow::anyhow;
use clap::Parser;
use rsnano_types::{Account, WalletId};

#[derive(Parser, PartialEq, Debug)]
pub(crate) struct SetWalletRepresentativeArgs {
    /// Sets the representative for the supplied <wallet>
    #[arg(long)]
    wallet: String,
    /// Sets the supplied account as the wallet representative
    #[arg(long)]
    account: String,
    /// Optional password to unlock the wallet
    #[arg(long)]
    password: Option<String>,
}

impl SetWalletRepresentativeArgs {
    pub(crate) fn set_representative_wallet(&self, global_args: GlobalArgs) -> anyhow::Result<()> {
        let node = build_node(&global_args)?;
        let wallet_id =
            WalletId::decode_hex(&self.wallet).ok_or_else(|| anyhow!("Invalid wallet id"))?;
        let representative = Account::parse(&self.account)
            .ok_or_else(|| anyhow!("Invalid account"))?
            .into();
        let password = self.password.clone().unwrap_or_default();

        node.services()
            .wallets
            .ensure_wallet_is_unlocked(wallet_id, &password);

        node.services()
            .wallets
            .set_representative(wallet_id, representative, false)
            .wait()
            .map_err(|e| anyhow!("Failed to set wallet representative: {:?}", e))?;

        Ok(())
    }
}
