use std::sync::Arc;

use anyhow::Result;

use crate::{environment::StoreEnvironmentOptions, wallet::WalletEnvironment};

/// Factory abstraction for constructing wallet database environments.
pub trait WalletEnvironmentFactory: Send + Sync {
    fn create_environment(
        &self,
        options: StoreEnvironmentOptions,
    ) -> Result<Arc<dyn WalletEnvironment>>;
}
