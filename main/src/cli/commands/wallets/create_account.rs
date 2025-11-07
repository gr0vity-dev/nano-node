use crate::cli::{GlobalArgs, build_node};
use anyhow::anyhow;
use clap::Parser;
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
    pub(crate) fn create_account(&self, global_args: GlobalArgs) -> anyhow::Result<()> {
        let node = build_node(&global_args)?;
        let wallet =
            WalletId::decode_hex(&self.wallet).ok_or_else(|| anyhow!("Invalid wallet id"))?;
        let password = self.password.clone().unwrap_or_default();

        node.services
            .wallets
            .ensure_wallet_is_unlocked(wallet, &password);

        let public_key = node
            .services
            .wallets
            .deterministic_insert2(&wallet, false)
            .map_err(|e| anyhow!("Failed to insert wallet: {:?}", e))?;

        println!("Account: {:?}", Account::from(public_key).encode_account());

        Ok(())
    }
}
