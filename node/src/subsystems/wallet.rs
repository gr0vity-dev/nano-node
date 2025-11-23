//! WalletSubsystem provides lifecycle integration for wallet services. Production APIs surface WalletServices; internal handles remain encapsulated.
use crate::WalletServices;

use super::lifecycle::Lifecycle;

/// Facade over wallet services to participate in node lifecycle.
#[derive(Clone)]
pub struct WalletSubsystem {
    services: WalletServices,
}

impl WalletSubsystem {
    pub fn new(services: WalletServices) -> Self {
        Self { services }
    }

    pub fn services(&self) -> WalletServices {
        self.services.clone()
    }
}

impl Lifecycle for WalletSubsystem {
    fn start(&mut self) {
        // Wallets have no dedicated start phase today.
    }

    fn stop(&mut self) {
        self.services.stop();
    }
}
