//! Narrow production handles for node-managed subsystems.
use std::sync::Arc;

use rsnano_ledger::{AnyReceivableIterator, AnySet, ConfirmedSet, Ledger, LedgerSet};
use rsnano_types::{
    Account, AccountInfo, Amount, BlockHash, ConfirmationHeightInfo, DetailedBlock, Link,
    PendingInfo, PendingKey, SavedBlock,
};

#[derive(Clone)]
pub struct ProductionHandles {
    ledger_info: LedgerInfoHandle,
    ledger_counts: LedgerCountsHandle,
    ledger_account_count: LedgerAccountCountHandle,
    ledger_account_balances: LedgerAccountBalanceHandle,
    ledger_work_thresholds: LedgerWorkThresholdHandle,
    ledger_state_checks: LedgerStateCheckHandle,
    ledger_queries: LedgerQueryHandle,
}

impl ProductionHandles {
    pub(crate) fn new(ledger: Arc<Ledger>) -> Self {
        let ledger_info = LedgerInfoHandle::new(ledger.clone());
        Self {
            ledger_info,
            ledger_counts: LedgerCountsHandle::new(ledger.clone()),
            ledger_account_count: LedgerAccountCountHandle::new(ledger.clone()),
            ledger_account_balances: LedgerAccountBalanceHandle::new(ledger.clone()),
            ledger_work_thresholds: LedgerWorkThresholdHandle::new(ledger.clone()),
            ledger_state_checks: LedgerStateCheckHandle::new(ledger.clone()),
            ledger_queries: LedgerQueryHandle::new(ledger),
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

    pub fn ledger_work_thresholds(&self) -> LedgerWorkThresholdHandle {
        self.ledger_work_thresholds.clone()
    }

    pub fn ledger_state_checks(&self) -> LedgerStateCheckHandle {
        self.ledger_state_checks.clone()
    }

    pub fn ledger_queries(&self) -> LedgerQueryHandle {
        self.ledger_queries.clone()
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

#[derive(Clone)]
pub struct LedgerWorkThresholdHandle {
    ledger: Arc<Ledger>,
}

impl LedgerWorkThresholdHandle {
    pub(crate) fn new(ledger: Arc<Ledger>) -> Self {
        Self { ledger }
    }

    pub fn threshold_base(&self) -> u64 {
        self.ledger.work_thresholds().threshold_base()
    }
}

#[derive(Clone)]
pub struct LedgerStateCheckHandle {
    ledger: Arc<Ledger>,
}

impl LedgerStateCheckHandle {
    pub(crate) fn new(ledger: Arc<Ledger>) -> Self {
        Self { ledger }
    }

    pub fn block_exists(&self, hash: &BlockHash) -> bool {
        self.ledger.any().block_exists(hash)
    }

    pub fn account_balance(&self, account: &Account) -> Amount {
        self.ledger.any().account_balance(account)
    }

    pub fn is_epoch_link(&self, link: &Link) -> bool {
        self.ledger.is_epoch_link(link)
    }
}

#[derive(Clone)]
pub struct LedgerQueryHandle {
    ledger: Arc<Ledger>,
}

impl LedgerQueryHandle {
    pub(crate) fn new(ledger: Arc<Ledger>) -> Self {
        Self { ledger }
    }

    pub fn account_info(&self, account: &Account) -> Option<AccountInfo> {
        self.ledger.any().get_account(account)
    }

    pub fn confirmation_height_info(&self, account: &Account) -> Option<ConfirmationHeightInfo> {
        self.ledger.confirmed().get_conf_info(account)
    }

    pub fn representative_block_hash(&self, head: &BlockHash) -> BlockHash {
        self.ledger.any().representative_block_hash(head)
    }

    pub fn block_balance(&self, hash: &BlockHash) -> Option<Amount> {
        self.ledger.any().block_balance(hash)
    }

    pub fn get_block(&self, hash: &BlockHash) -> Option<SavedBlock> {
        self.ledger.any().get_block(hash)
    }

    pub fn detailed_block(&self, hash: &BlockHash) -> Option<DetailedBlock> {
        self.ledger.any().detailed_block(hash)
    }

    pub fn linked_account(&self, block: &SavedBlock) -> Option<Account> {
        self.ledger.any().linked_account(block)
    }

    pub fn block_successor(&self, hash: &BlockHash) -> Option<BlockHash> {
        self.ledger.any().block_successor(hash)
    }

    pub fn block_exists(&self, hash: &BlockHash) -> bool {
        self.ledger.any().block_exists(hash)
    }

    pub fn weight_exact(&self, account: Account) -> Amount {
        self.ledger.any().weight_exact(account.into())
    }

    pub fn account_receivable(&self, account: &Account) -> Amount {
        self.ledger.any().account_receivable(account)
    }

    pub fn confirmed_account_receivable(&self, account: &Account) -> Amount {
        self.ledger.confirmed().account_receivable(account)
    }

    pub fn account_head(&self, account: &Account) -> Option<BlockHash> {
        self.ledger.any().account_head(account)
    }

    pub fn confirmed_block_exists(&self, hash: &BlockHash) -> bool {
        self.ledger.confirmed().block_exists(hash)
    }

    pub fn block_amount(&self, hash: &BlockHash) -> Option<Amount> {
        self.ledger.any().block_amount(hash)
    }

    pub fn find_receive_block_by_send_hash(
        &self,
        destination: &Account,
        send_block_hash: &BlockHash,
    ) -> Option<SavedBlock> {
        self.ledger
            .any()
            .find_receive_block_by_send_hash(destination, send_block_hash)
    }

    pub fn get_pending(&self, key: &PendingKey) -> Option<PendingInfo> {
        self.ledger.any().get_pending(key)
    }

    pub fn receivable_upper_bound(
        &self,
        account: Account,
        start: BlockHash,
    ) -> Vec<(PendingKey, PendingInfo)> {
        self.ledger
            .any()
            .account_receivable_upper_bound(account, start)
            .collect()
    }

    pub fn pending_from(&self, start: PendingKey) -> Vec<(PendingKey, PendingInfo)> {
        self.ledger
            .any()
            .iter_pending_range(start..)
            .collect()
    }
}
