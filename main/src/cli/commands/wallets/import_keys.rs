use std::{fs::File, io::Read, path::PathBuf};

use anyhow::{Context, anyhow};
use clap::Parser;

use rsnano_types::WalletId;

use rsnano_node::services::WalletServices;

#[derive(Parser, PartialEq, Debug)]
pub(crate) struct ImportKeysArgs {
    /// The path of the file that contains the keys
    #[arg(long)]
    file: String,
    #[arg(long)]
    /// Optional password to unlock the wallet
    password: Option<String>,
    #[arg(long)]
    /// Forces the command if the wallet is locked
    force: bool,
    /// The wallet importing the keys
    #[arg(long)]
    wallet: String,
}

impl ImportKeysArgs {
    pub(crate) fn import_keys(&self, wallet_services: &WalletServices) -> anyhow::Result<()> {
        let mut file = File::open(PathBuf::from(&self.file))?;
        let mut contents = String::new();

        file.read_to_string(&mut contents)
            .context("Unable to read <file> contents")?;

        let wallet_id =
            WalletId::decode_hex(&self.wallet).ok_or_else(|| anyhow!("Invalid wallet id"))?;
        let password = self.password.clone().unwrap_or_default();

        wallet_services.ensure_wallet_is_unlocked(wallet_id, &password);

        if wallet_services.wallet_exists(&wallet_id) {
            let valid = wallet_services.ensure_wallet_is_unlocked(wallet_id, &password);
            if valid {
                wallet_services.import_replace(wallet_id, &contents, &password)?
            } else {
                eprintln!(
                    "Invalid password for wallet {}. New wallet should have empty (default) password or passwords for new wallet & json file should match",
                    wallet_id
                );
                return Err(anyhow!("Invalid arguments"));
            }
        } else if !self.force {
            eprintln!("Wallet doesn't exist");
            return Err(anyhow!("Invalid arguments"));
        } else {
            wallet_services.import_wallet(wallet_id, &contents)?
        }

        Ok(())
    }
}
