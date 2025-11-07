use crate::cli::{GlobalArgs, build_node};
use anyhow::anyhow;
use clap::Parser;
use rsnano_types::{RawKey, WalletId};

#[derive(Parser, PartialEq, Debug)]
pub(crate) struct AddPrivateKeyArgs {
    /// Adds the key to the supplied wallet
    #[arg(long)]
    wallet: String,
    /// Adds the supplied <private_key> to the wallet
    #[arg(long)]
    private_key: String,
    /// Optional <password> to unlock the wallet
    #[arg(long)]
    password: Option<String>,
}

impl AddPrivateKeyArgs {
    pub(crate) fn add_key(&self, global_args: GlobalArgs) -> anyhow::Result<()> {
        let node = build_node(&global_args)?;
        let wallet_id =
            WalletId::decode_hex(&self.wallet).ok_or_else(|| anyhow!("Invalid wallet id"))?;
        let private_key =
            RawKey::decode_hex(&self.private_key).ok_or_else(|| anyhow!("Invalid private key"))?;
        let password = self.password.clone().unwrap_or_default();
        node.services()
            .wallets
            .ensure_wallet_is_unlocked(wallet_id, &password);

        node.services()
            .wallets
            .insert_adhoc2(&wallet_id, &private_key, false)
            .map_err(|e| anyhow!("Failed to insert key: {:?}", e))?;

        Ok(())
    }
}
