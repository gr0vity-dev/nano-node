mod add_private_key;
mod change_wallet_seed;
mod create_account;
mod create_wallet;
mod decrypt_wallet;
mod destroy_wallet;
mod get_wallet_representative;
mod import_keys;
mod remove_account;
mod set_wallet_representative;

use crate::cli::{GlobalArgs, build_node};
use add_private_key::AddPrivateKeyArgs;
use anyhow::{Result, anyhow};
use change_wallet_seed::ChangeWalletSeedArgs;
use clap::{CommandFactory, Parser, Subcommand};
use create_account::CreateAccountArgs;
use create_wallet::CreateWalletArgs;
use decrypt_wallet::DecryptWalletArgs;
use destroy_wallet::DestroyWalletArgs;
use get_wallet_representative::GetWalletRepresentativeArgs;
use import_keys::ImportKeysArgs;
use remove_account::RemoveAccountArgs;
use rsnano_node::services::WalletServices;
use rsnano_types::Account;
use set_wallet_representative::SetWalletRepresentativeArgs;

#[derive(Parser, PartialEq, Debug)]
pub(crate) struct WalletsCommand {
    #[command(subcommand)]
    pub subcommand: Option<WalletSubcommands>,
}

#[derive(Subcommand, PartialEq, Debug)]
pub(crate) enum WalletSubcommands {
    /// Creates a new account in a wallet
    CreateAccount(CreateAccountArgs),
    /// Creates a new wallet
    CreateWallet(CreateWalletArgs),
    /// Destroys a wallet
    Destroy(DestroyWalletArgs),
    /// Imports keys from a file to a wallet
    ImportKeys(ImportKeysArgs),
    /// Adds a private_key to a wallet
    AddPrivateKey(AddPrivateKeyArgs),
    /// Changes the seed of a wallet
    ChangeWalletSeed(ChangeWalletSeedArgs),
    /// Prints the representative of a wallet
    GetWalletRepresentative(GetWalletRepresentativeArgs),
    /// Sets the representative of a wallet
    SetWalletRepresentative(SetWalletRepresentativeArgs),
    /// Removes an account from a wallet
    RemoveAccount(RemoveAccountArgs),
    /// Decrypts a wallet (WARNING: THIS WILL PRINT YOUR PRIVATE KEY TO STDOUT!)
    DecryptWallet(DecryptWalletArgs),
    /// List all wallets and their public keys
    List,
    /// Removes all send IDs from the wallets (dangerous: not intended for production use)
    ClearSendIds,
}

pub(crate) fn run_wallets_command(global_args: GlobalArgs, cmd: WalletsCommand) -> Result<()> {
    match cmd.subcommand {
        Some(WalletSubcommands::List) => {
            with_wallet_services(&global_args, |services| list_wallets(services))?
        }
        Some(WalletSubcommands::CreateWallet(args)) => {
            with_wallet_services(&global_args, |services| args.create_wallet(services))?
        }
        Some(WalletSubcommands::CreateAccount(args)) => {
            with_wallet_services(&global_args, |services| args.create_account(services))?
        }
        Some(WalletSubcommands::Destroy(args)) => {
            with_wallet_services(&global_args, |services| args.destroy_wallet(services))?
        }
        Some(WalletSubcommands::AddPrivateKey(args)) => {
            with_wallet_services(&global_args, |services| args.add_key(services))?
        }
        Some(WalletSubcommands::ChangeWalletSeed(args)) => {
            with_wallet_services(&global_args, |services| args.change_wallet_seed(services))?
        }
        Some(WalletSubcommands::ImportKeys(args)) => {
            with_wallet_services(&global_args, |services| args.import_keys(services))?
        }
        Some(WalletSubcommands::RemoveAccount(args)) => {
            with_wallet_services(&global_args, |services| args.remove_account(services))?
        }
        Some(WalletSubcommands::DecryptWallet(args)) => {
            with_wallet_services(&global_args, |services| args.decrypt_wallet(services))?
        }
        Some(WalletSubcommands::GetWalletRepresentative(args)) => {
            with_wallet_services(&global_args, |services| {
                args.get_wallet_representative(services)
            })?
        }
        Some(WalletSubcommands::SetWalletRepresentative(args)) => {
            with_wallet_services(&global_args, |services| {
                args.set_representative_wallet(services)
            })?
        }
        Some(WalletSubcommands::ClearSendIds) => {
            with_wallet_services(&global_args, |services| clear_send_ids(services))?
        }
        None => WalletsCommand::command().print_long_help()?,
    }

    Ok(())
}

impl WalletsCommand {}

fn list_wallets(wallet_services: &WalletServices) -> Result<()> {
    let wallet_ids = wallet_services.wallet_ids();

    for wallet_id in wallet_ids {
        println!("{:?}", wallet_id);
        let accounts = wallet_services
            .accounts_of_wallet(&wallet_id)
            .map_err(|e| anyhow!("Failed to get accounts of wallets: {:?}", e))?;
        if !accounts.is_empty() {
            for account in accounts {
                println!("{:?}", Account::encode_account(&account));
            }
        }
    }

    Ok(())
}

fn clear_send_ids(wallet_services: &WalletServices) -> anyhow::Result<()> {
    wallet_services.clear_send_ids();
    println!("Send IDs deleted");
    Ok(())
}

fn with_wallet_services<F>(global_args: &GlobalArgs, f: F) -> anyhow::Result<()>
where
    F: FnOnce(&WalletServices) -> anyhow::Result<()>,
{
    let node = build_node(global_args)?;
    let wallet_services = node.wallet_services();
    drop(node);
    f(&wallet_services)
}
