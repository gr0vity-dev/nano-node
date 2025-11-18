use std::sync::Arc;

use crate::{WalletEnvHandle, WalletStoreFactory};

/// Helper harness that keeps wallet environment handles and store factories packaged for tests.
pub struct WalletEnvTestHarness {
    env_handle: Arc<WalletEnvHandle>,
    store_factory: Arc<dyn WalletStoreFactory>,
}

impl WalletEnvTestHarness {
    pub fn new(
        env_handle: Arc<WalletEnvHandle>,
        store_factory: Arc<dyn WalletStoreFactory>,
    ) -> Self {
        Self {
            env_handle,
            store_factory,
        }
    }

    pub fn env(&self) -> Arc<WalletEnvHandle> {
        Arc::clone(&self.env_handle)
    }

    pub fn store_factory(&self) -> Arc<dyn WalletStoreFactory> {
        Arc::clone(&self.store_factory)
    }
}
