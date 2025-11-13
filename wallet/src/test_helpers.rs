use std::sync::Arc;

use rsnano_store_lmdb::LmdbWalletEnvironment;
use rsnano_types::KeyDerivationFunction;

use crate::{WalletEnvHandle, WalletStoreFactory, WalletsConfig};

/// Helper harness that provisions a wallet environment + store factory for tests
/// while exposing only the trait objects the logic layer expects.
pub struct WalletEnvTestHarness {
    env_handle: Arc<WalletEnvHandle>,
    store_factory: Arc<dyn WalletStoreFactory>,
}

impl WalletEnvTestHarness {
    pub fn new(password_fanout: usize, kdf: KeyDerivationFunction) -> Self {
        let env_impl = Arc::new(
            LmdbWalletEnvironment::new_null()
                .expect("Failed to initialize wallet LMDB environment for tests"),
        );
        let env_handle: Arc<WalletEnvHandle> = env_impl.clone();
        let store_factory: Arc<dyn WalletStoreFactory> =
            Arc::new(env_impl.create_store_factory(password_fanout, kdf));

        Self {
            env_handle,
            store_factory,
        }
    }

    pub fn from_config(config: &WalletsConfig) -> Self {
        Self::new(
            config.password_fanout as usize,
            KeyDerivationFunction::new(config.kdf_work),
        )
    }

    pub fn env(&self) -> Arc<WalletEnvHandle> {
        Arc::clone(&self.env_handle)
    }

    pub fn store_factory(&self) -> Arc<dyn WalletStoreFactory> {
        Arc::clone(&self.store_factory)
    }
}
