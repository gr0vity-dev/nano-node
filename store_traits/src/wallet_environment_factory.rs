use std::sync::Arc;

use anyhow::Result;
use rsnano_types::KeyDerivationFunction;

use crate::{
    environment::StoreEnvironmentOptions,
    wallet::{WalletEnvironment, WalletStoreFactory},
};

pub struct WalletEnvironmentBundle {
    pub environment: Arc<dyn WalletEnvironment>,
    pub store_factory: Arc<dyn WalletStoreFactory>,
}

/// Factory abstraction for constructing wallet database environments.
pub trait WalletEnvironmentFactory: Send + Sync {
    fn create(
        &self,
        options: StoreEnvironmentOptions,
        fanout: usize,
        kdf: KeyDerivationFunction,
    ) -> Result<WalletEnvironmentBundle>;
}
