use crate::cli::{GlobalArgs, build_node};
use anyhow::anyhow;
use clap::Parser;
use rsnano_types::{Account, WalletId};

#[derive(Parser, PartialEq, Debug)]
pub(crate) struct RemoveAccountArgs {
    /// Removes the account from the supplied wallet
    #[arg(long)]
    wallet: String,
    /// Removes the account from the supplied wallet
    #[arg(long)]
    account: String,
    /// Optional password to unlock the wallet
    #[arg(long)]
    password: Option<String>,
}

impl RemoveAccountArgs {
    pub(crate) fn remove_account(&self, global_args: GlobalArgs) -> anyhow::Result<()> {
        let node = build_node(&global_args)?;
        let wallet_id =
            WalletId::decode_hex(&self.wallet).ok_or_else(|| anyhow!("Invalid wallet id"))?;
        let password = self.password.clone().unwrap_or_default();
        let account = Account::parse(&self.account)
            .ok_or_else(|| anyhow!("Invalid account"))?
            .into();

        node.services()
            .wallets
            .ensure_wallet_is_unlocked(wallet_id, &password);

        node.services()
            .wallets
            .remove_key(&wallet_id, &account)
            .map_err(|e| anyhow!("Failed to remove account: {:?}", e))?;

        Ok(())
    }
}
