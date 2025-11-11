use std::sync::Arc;

use rsnano_ledger::{AnySet, Ledger};
use rsnano_nullable_lmdb::{Transaction, WriteTransaction};
use rsnano_types::{PrivateKey, PublicKey, WalletId, WorkNonce};

use crate::WalletStore;

pub struct Wallet {
    id: WalletId,
    pub store: Arc<dyn WalletStore>,
}

impl Wallet {
    pub fn id(&self) -> &WalletId {
        &self.id
    }

    pub fn from_store(id: WalletId, store: Arc<dyn WalletStore>) -> Self {
        Self { id, store }
    }

    pub fn work_put(&self, txn: &mut WriteTransaction, pub_key: &PublicKey, work: WorkNonce) {
        self.store.work_put(txn, pub_key, work);
    }

    pub fn deterministic_check(&self, txn: &dyn Transaction, index: u32, ledger: &Ledger) -> u32 {
        let mut result = index;
        let any = ledger.any();
        let mut i = index + 1;
        let mut n = index + 64;
        while i < n {
            let prv = self.store.deterministic_key(txn, i);
            let pair = PrivateKey::from_bytes(prv.as_bytes());
            // Check if account received at least 1 block
            let latest = any.account_head(&pair.account());
            match latest {
                Some(_) => {
                    result = i;
                    // i + 64 - Check additional 64 accounts
                    // i/64 - Check additional accounts for large wallets. I.e. 64000/64 = 1000 accounts to check
                    n = i + 64 + (i / 64);
                }
                None => {
                    // Check if there are pending blocks for account
                    if any.receivable_exists(pair.account()) {
                        result = i;
                        n = i + 64 + (i / 64);
                    }
                }
            }

            i += 1;
        }
        result
    }

    pub fn live(&self) -> bool {
        self.store.is_open()
    }
}
