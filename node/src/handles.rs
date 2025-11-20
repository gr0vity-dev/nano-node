//! Narrow production handles for node-managed subsystems.
use std::sync::Arc;

use rsnano_ledger::{Ledger, LedgerSet};
use rsnano_types::{Account, Amount};

#[derive(Clone)]
pub struct ProductionHandles {
    ledger_info: LedgerInfoHandle,
    ledger_counts: LedgerCountsHandle,
    ledger_account_count: LedgerAccountCountHandle,
    ledger_account_balances: LedgerAccountBalanceHandle,
}

impl ProductionHandles {
    pub(crate) fn new(ledger: Arc<Ledger>) -> Self {
        let ledger_info = LedgerInfoHandle::new(ledger.clone());
        Self {
            ledger_info,
            ledger_counts: LedgerCountsHandle::new(ledger.clone()),
            ledger_account_count: LedgerAccountCountHandle::new(ledger.clone()),
            ledger_account_balances: LedgerAccountBalanceHandle::new(ledger),
        }
    }

    pub fn ledger_info(&self) -> LedgerInfoHandle {
        self.ledger_info.clone()
    }

    pub fn ledger_counts(&self) -> LedgerCountsHandle {
        self.ledger_counts.clone()
    }

    pub fn ledger_account_count(&self) -> LedgerAccountCountHandle {
        self.ledger_account_count.clone()
    }

    pub fn ledger_account_balances(&self) -> LedgerAccountBalanceHandle {
        self.ledger_account_balances.clone()
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

#[derive(Clone)]
pub struct LedgerAccountCountHandle {
    ledger: Arc<Ledger>,
}

impl LedgerAccountCountHandle {
    pub(crate) fn new(ledger: Arc<Ledger>) -> Self {
        Self { ledger }
    }

    pub fn account_count(&self) -> u64 {
        self.ledger.account_count()
    }
}

#[derive(Clone)]
pub struct LedgerAccountBalanceHandle {
    ledger: Arc<Ledger>,
}

impl LedgerAccountBalanceHandle {
    pub(crate) fn new(ledger: Arc<Ledger>) -> Self {
        Self { ledger }
    }

    pub fn confirmed_balance_and_receivable(&self, account: &Account) -> (Amount, Amount) {
        let confirmed = self.ledger.confirmed();
        (
            confirmed.account_balance(account),
            confirmed.account_receivable(account),
        )
    }

    pub fn any_balance_and_receivable(&self, account: &Account) -> (Amount, Amount) {
        let any = self.ledger.any();
        (
            any.account_balance(account),
            any.account_receivable(account),
        )
    }

    pub fn account_block_count(&self, account: &Account) -> Option<u64> {
        self.ledger
            .any()
            .get_account(account)
            .map(|info| info.block_count)
    }
}
