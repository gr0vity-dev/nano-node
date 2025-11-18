use std::sync::{Arc, Mutex};

#[cfg(test)]
use rsnano_ledger::Ledger;
use rsnano_ledger::RepWeightCache;
#[cfg(test)]
use rsnano_nullable_clock::SteadyClock;
#[cfg(test)]
use rsnano_store_lmdb::{LmdbWalletEnvironment, null_ledger_store_factory};
use rsnano_types::{Account, Amount, PrivateKey, PublicKey};
#[cfg(test)]
use rsnano_types::{KeyDerivationFunction, Networks};
use rsnano_utils::{CancellationToken, ticker::Tickable};
use rsnano_wallet::Wallets;
#[cfg(test)]
use rsnano_wallet::{WalletEnvHandle, WalletStoreFactory, WalletsConfig};
#[cfg(test)]
use rsnano_work_validation::WorkThresholds;

use crate::representatives::OnlineReps;

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
    online_reps: Arc<Mutex<OnlineReps>>,
}

impl WalletRepresentatives {
    pub fn new(
        voting_enabled: bool,
        vote_minimum: Amount,
        rep_weights: Arc<RepWeightCache>,
        wallets: Arc<Wallets>,
        online_reps: Arc<Mutex<OnlineReps>>,
    ) -> Self {
        Self {
            voting_enabled,
            half_principal: false,
            rep_keys: Vec::new(),
            vote_minimum,
            rep_weights,
            wallets,
            online_reps,
        }
    }

    #[cfg(test)]
    pub fn new_null() -> Self {
        Self::new(
            false,
            Amount::ZERO,
            Arc::new(RepWeightCache::new()),
            Arc::new(create_test_wallets()),
            Arc::new(Mutex::new(OnlineReps::new_test_instance())),
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
        let half_principal_weight = self.online_reps.lock().unwrap().minimum_principal_weight() / 2;
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

#[cfg(test)]
fn create_test_wallets() -> Wallets {
    let network = Networks::NanoLiveNetwork;
    let ledger = Arc::new(Ledger::new_null(null_ledger_store_factory()));
    let wallets_config = WalletsConfig::default();
    let work = WorkThresholds::default_for(network);
    let clock = Arc::new(SteadyClock::new_null());
    let env_impl = Arc::new(
        LmdbWalletEnvironment::new_null().expect("Failed to initialize wallet LMDB environment"),
    );
    let wallet_env: Arc<WalletEnvHandle> = env_impl.clone();
    let store_factory: Arc<dyn WalletStoreFactory> = Arc::new(env_impl.create_store_factory(
        wallets_config.password_fanout,
        KeyDerivationFunction::new(wallets_config.kdf_work),
    ));

    Wallets::new(
        wallets_config,
        wallet_env,
        ledger,
        work,
        clock,
        store_factory,
    )
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
