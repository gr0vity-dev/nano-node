#[macro_use]
extern crate anyhow;

mod config;
mod delayed_work_queue;
mod promises;
mod receivable_search;
mod store_traits;
mod wallet;
mod wallet_backup;
mod wallets;

use serde::{Deserialize, Serialize};

pub use config::{WalletsConfig, default_preconfigured_representatives_for_live};
pub use promises::*;
pub use receivable_search::ReceivableSearch;
pub use rsnano_store_lmdb::LmdbWalletEnvironment;
pub use store_traits::*;
pub use wallet::Wallet;
pub use wallet_backup::WalletBackup;
pub use wallets::{WalletEnvHandle, Wallets, WalletsTicker};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub enum WalletsError {
    Generic,
    WalletNotFound,
    WalletLocked,
    AccountNotFound,
    InvalidPassword,
    BadPublicKey,
}

impl WalletsError {
    pub fn as_str(&self) -> &'static str {
        match self {
            WalletsError::Generic => "Unknown error",
            WalletsError::WalletNotFound => "Wallet not found",
            WalletsError::WalletLocked => "Wallet is locked",
            WalletsError::AccountNotFound => "Account not found",
            WalletsError::InvalidPassword => "Invalid password",
            WalletsError::BadPublicKey => "Bad public key",
        }
    }
}

impl std::fmt::Display for WalletsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::error::Error for WalletsError {}
