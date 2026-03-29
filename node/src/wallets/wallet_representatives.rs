use std::sync::{Arc, Mutex};

use rsnano_ledger::RepWeightCache;
use rsnano_types::{Account, Amount, PrivateKey, PublicKey};
use rsnano_utils::{CancellationToken, ticker::Tickable};
use rsnano_wallet::Wallets;

use crate::representatives::{OnlineReps, VoteQuorumPreparer};

#[derive(Clone)]
pub struct WalletRepresentatives {
    voting_enabled: bool,
    /// has representatives with at least 50% of principal representative requirements
    half_principal: bool,
    /// Representatives with at least the configured minimum voting weight
    rep_keys: Vec<PublicKey>,
    vote_minimum: Amount,
    rep_weights: Arc<RepWeightCache>,
    wallets: Arc<Wallets>,
    quorum_preparer: Arc<VoteQuorumPreparer>,
}

impl WalletRepresentatives {
    pub fn new(
        voting_enabled: bool,
        vote_minimum: Amount,
        rep_weights: Arc<RepWeightCache>,
        wallets: Arc<Wallets>,
        quorum_preparer: Arc<VoteQuorumPreparer>,
    ) -> Self {
        Self {
            voting_enabled,
            half_principal: false,
            rep_keys: Vec::new(),
            vote_minimum,
            rep_weights,
            wallets,
            quorum_preparer,
        }
    }

    pub fn new_null() -> Self {
        let online_reps = Arc::new(Mutex::new(OnlineReps::new_test_instance()));
        Self::new(
            false,
            Amount::ZERO,
            Arc::new(RepWeightCache::default()),
            Arc::new(Wallets::new_null()),
            Arc::new(VoteQuorumPreparer::new(online_reps)),
        )
    }

    pub fn have_half_rep(&self) -> bool {
        self.half_principal
    }

    #[cfg(test)]
    pub fn set_have_half_rep(&mut self, value: bool) {
        self.half_principal = value;
    }

    pub fn voting_enabled(&self) -> bool {
        self.voting_enabled && self.voting_reps() > 0
    }

    pub fn voting_reps(&self) -> usize {
        self.rep_keys.len()
    }

    pub fn exists(&self, rep: &Account) -> bool {
        self.rep_keys.iter().any(|k| k.as_account() == *rep)
    }

    pub fn rep_pub_keys(&self) -> impl Iterator<Item = PublicKey> + use<'_> {
        self.rep_keys.iter().cloned()
    }

    pub fn rep_priv_keys(&self, result: &mut Vec<PrivateKey>) {
        result.clear();

        if !self.voting_enabled {
            return;
        }

        let all_priv_keys = self.wallets.get_all_private_keys();
        {
            for rep_key in self.rep_pub_keys() {
                if let Some(k) = all_priv_keys.iter().find(|k| k.public_key() == rep_key) {
                    result.push(k.clone());
                }
            }
        }
    }

    pub fn rep_accounts(&self) -> impl Iterator<Item = Account> + use<'_> {
        self.rep_keys.iter().map(|k| k.as_account())
    }

    pub fn clear(&mut self) {
        self.half_principal = false;
        self.rep_keys.clear();
    }

    pub fn compute_reps(&mut self) {
        let half_principal_weight = self.quorum_preparer.minimum_principal_weight() / 2;
        let wallet_keys = self.wallets.get_all_pub_keys();
        self.clear();
        for pub_key in wallet_keys {
            self.check_rep(pub_key, half_principal_weight);
        }
    }

    pub fn check_rep(&mut self, pub_key: PublicKey, half_principal_weight: Amount) -> bool {
        let weight = self.rep_weights.weight(&pub_key);

        if weight < self.vote_minimum {
            return false; // account not a representative
        }

        if weight >= half_principal_weight {
            self.half_principal = true;
        }

        self.insert(pub_key)
    }

    fn insert(&mut self, pub_key: impl Into<PublicKey>) -> bool {
        let rep_key = pub_key.into();
        if self.rep_keys.contains(&rep_key) {
            return false;
        }

        self.rep_keys.push(rep_key);
        true
    }
}

pub(crate) struct LocalRepsComputation(Arc<Mutex<WalletRepresentatives>>);

impl LocalRepsComputation {
    pub fn new(wallet_reps: Arc<Mutex<WalletRepresentatives>>) -> Self {
        Self(wallet_reps)
    }
}

impl Tickable for LocalRepsComputation {
    fn tick(&mut self, _: &CancellationToken) {
        self.0.lock().unwrap().compute_reps();
    }
}
