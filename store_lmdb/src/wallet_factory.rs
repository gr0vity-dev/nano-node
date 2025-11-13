use std::sync::Arc;

use anyhow::Result;
use rsnano_nullable_lmdb::{LmdbDatabase, LmdbEnvironment, Transaction};
use rsnano_types::{KeyDerivationFunction, PublicKey, WalletId};
use store_traits::wallet::WalletStoreFactory;

use crate::{LmdbIterator, LmdbWalletStore};

pub struct LmdbWalletStoreFactory {
    env: Arc<LmdbEnvironment>,
    fanout: usize,
    kdf: KeyDerivationFunction,
}

impl LmdbWalletStoreFactory {
    pub fn new(env: Arc<LmdbEnvironment>, fanout: usize, kdf: KeyDerivationFunction) -> Self {
        Self { env, fanout, kdf }
    }

    fn wallet_path(&self, wallet_id: WalletId) -> std::path::PathBuf {
        std::path::PathBuf::from(wallet_id.to_string())
    }

    fn wallets_db(&self) -> LmdbDatabase {
        self.env.open_db(None).expect("wallets db should exist")
    }
}

impl WalletStoreFactory for LmdbWalletStoreFactory {
    fn open_existing(
        &self,
        wallet_id: WalletId,
    ) -> Result<Arc<dyn store_traits::wallet::WalletStore>> {
        let path = self.wallet_path(wallet_id);
        let store = LmdbWalletStore::new(
            self.fanout,
            self.kdf.clone(),
            &self.env,
            &PublicKey::ZERO,
            &path,
        )?;
        Ok(Arc::new(store))
    }

    fn create_new(
        &self,
        wallet_id: WalletId,
        representative: PublicKey,
    ) -> Result<Arc<dyn store_traits::wallet::WalletStore>> {
        let path = self.wallet_path(wallet_id);
        let store = LmdbWalletStore::new(
            self.fanout,
            self.kdf.clone(),
            &self.env,
            &representative,
            &path,
        )?;
        Ok(Arc::new(store))
    }

    fn create_from_json(
        &self,
        wallet_id: WalletId,
        json: &str,
    ) -> Result<Arc<dyn store_traits::wallet::WalletStore>> {
        let path = self.wallet_path(wallet_id);
        let store =
            LmdbWalletStore::new_from_json(self.fanout, self.kdf.clone(), &self.env, &path, json)?;
        Ok(Arc::new(store))
    }

    fn list_wallet_ids(&self) -> Result<Vec<WalletId>> {
        let db = self.wallets_db();
        let txn = self.env.begin_read();
        let cursor = txn.open_ro_cursor(db)?;
        let ids = LmdbIterator::new(cursor, |key, _| {
            if key.len() == 64 {
                let id =
                    WalletId::decode_hex(std::str::from_utf8(key).unwrap()).unwrap_or_default();
                (id, ())
            } else {
                (WalletId::ZERO, ())
            }
        })
        .filter_map(|(id, _)| if id.is_zero() { None } else { Some(id) })
        .collect();
        Ok(ids)
    }
}
