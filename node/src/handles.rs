//! Narrow production handles for node-managed subsystems.
use std::sync::Arc;

use rsnano_ledger::Ledger;

#[derive(Clone)]
pub struct ProductionHandles {
    ledger_info: LedgerInfoHandle,
}

impl ProductionHandles {
    pub(crate) fn new(ledger: Arc<Ledger>) -> Self {
        Self {
            ledger_info: LedgerInfoHandle::new(ledger),
        }
    }

    pub fn ledger_info(&self) -> LedgerInfoHandle {
        self.ledger_info.clone()
    }
}

#[derive(Clone)]
pub struct LedgerInfoHandle {
    ledger: Arc<Ledger>,
}

impl LedgerInfoHandle {
    pub(crate) fn new(ledger: Arc<Ledger>) -> Self {
        Self { ledger }
    }

    pub fn store_version(&self) -> u32 {
        self.ledger.version()
    }

    pub fn store_vendor(&self) -> String {
        self.ledger.store_vendor()
    }
}
