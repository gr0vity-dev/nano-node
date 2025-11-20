//! Narrow production handles for node-managed subsystems.
use std::sync::Arc;

use rsnano_ledger::Ledger;

#[derive(Clone)]
pub struct ProductionHandles {
    ledger_info: LedgerInfoHandle,
    ledger_counts: LedgerCountsHandle,
}

impl ProductionHandles {
    pub(crate) fn new(ledger: Arc<Ledger>) -> Self {
        let ledger_info = LedgerInfoHandle::new(ledger.clone());
        Self {
            ledger_info,
            ledger_counts: LedgerCountsHandle::new(ledger),
        }
    }

    pub fn ledger_info(&self) -> LedgerInfoHandle {
        self.ledger_info.clone()
    }

    pub fn ledger_counts(&self) -> LedgerCountsHandle {
        self.ledger_counts.clone()
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

#[derive(Clone)]
pub struct LedgerCountsHandle {
    ledger: Arc<Ledger>,
}

impl LedgerCountsHandle {
    pub(crate) fn new(ledger: Arc<Ledger>) -> Self {
        Self { ledger }
    }

    pub fn block_count(&self) -> u64 {
        self.ledger.block_count()
    }

    pub fn confirmed_count(&self) -> u64 {
        self.ledger.confirmed_count()
    }
}
