use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use rsnano_nullable_lmdb::{LmdbEnvironment, Transaction, WriteTransaction};
use rsnano_store_lmdb::{KeyType, LmdbWalletStore, WalletValue};
use rsnano_types::{KeyDerivationFunction, PublicKey, RawKey, WalletId, WorkNonce};

pub type WalletStoreIterator<'a> = Box<dyn Iterator<Item = (PublicKey, WalletValue)> + 'a>;

pub trait WalletStore: Send + Sync {
    fn password(&self) -> RawKey;
    fn valid_password(&self, txn: &dyn Transaction) -> bool;
    fn attempt_password(&self, txn: &dyn Transaction, password: &str) -> bool;
    fn rekey(&self, txn: &mut WriteTransaction, password: &str) -> Result<()>;
    fn lock(&self);
    fn is_open(&self) -> bool;
    fn deterministic_key(&self, txn: &dyn Transaction, index: u32) -> RawKey;
    fn deterministic_insert(&self, txn: &mut WriteTransaction) -> PublicKey;
    fn deterministic_insert_at(&self, txn: &mut WriteTransaction, index: u32) -> PublicKey;
    fn deterministic_index_get(&self, txn: &dyn Transaction) -> u32;
    fn insert_adhoc(&self, txn: &mut WriteTransaction, prv: &RawKey) -> PublicKey;
    fn insert_watch(&self, txn: &mut WriteTransaction, pub_key: &PublicKey) -> Result<()>;
    fn fetch(&self, txn: &dyn Transaction, pub_key: &PublicKey) -> Result<RawKey>;
    fn erase(&self, txn: &mut WriteTransaction, pub_key: &PublicKey);
    fn exists(&self, txn: &dyn Transaction, pub_key: &PublicKey) -> bool;
    fn find(&self, txn: &dyn Transaction, pub_key: &PublicKey) -> Option<WalletValue>;
    fn get_key_type(&self, txn: &dyn Transaction, pub_key: &PublicKey) -> KeyType;
    fn representative(&self, txn: &dyn Transaction) -> PublicKey;
    fn representative_set(&self, txn: &mut WriteTransaction, representative: &PublicKey);
    fn work_get(&self, txn: &dyn Transaction, pub_key: &PublicKey) -> Result<WorkNonce>;
    fn work_put(&self, txn: &mut WriteTransaction, pub_key: &PublicKey, work: WorkNonce);
    fn seed(&self, txn: &dyn Transaction) -> RawKey;
    fn set_seed(&self, txn: &mut WriteTransaction, seed: &RawKey);
    fn serialize_json(&self, txn: &dyn Transaction) -> String;
    fn write_backup(&self, txn: &dyn Transaction, path: &Path) -> Result<()>;
    fn iter<'a>(&'a self, txn: &'a dyn Transaction) -> WalletStoreIterator<'a>;
    fn destroy(&self, txn: &mut WriteTransaction);

    fn import_wallet(&self, txn: &mut WriteTransaction, other: &dyn WalletStore) -> Result<()> {
        assert!(self.valid_password(txn));
        assert!(other.valid_password(txn));

        enum ImportKey {
            Private((PublicKey, RawKey)),
            WatchOnly(PublicKey),
        }

        let mut keys = Vec::new();
        for (pub_key, _) in other.iter(txn) {
            match other.fetch(txn, &pub_key) {
                Ok(prv) if prv != RawKey::ZERO => keys.push(ImportKey::Private((pub_key, prv))),
                _ => keys.push(ImportKey::WatchOnly(pub_key)),
            }
        }

        for key in keys {
            match key {
                ImportKey::Private((pub_key, prv)) => {
                    let inserted = self.insert_adhoc(txn, &prv);
                    debug_assert_eq!(inserted, pub_key);
                    other.erase(txn, &pub_key);
                }
                ImportKey::WatchOnly(pub_key) => {
                    self.insert_watch(txn, &pub_key)?;
                    other.erase(txn, &pub_key);
                }
            }
        }

        Ok(())
    }
}

pub trait WalletStoreFactory: Send + Sync {
    fn open_existing(&self, wallet_id: WalletId) -> Result<Arc<dyn WalletStore>>;
    fn create_new(
        &self,
        wallet_id: WalletId,
        representative: PublicKey,
    ) -> Result<Arc<dyn WalletStore>>;
    fn create_from_json(&self, wallet_id: WalletId, json: &str) -> Result<Arc<dyn WalletStore>>;
}

pub struct LmdbWalletStoreFactory {
    env: Arc<LmdbEnvironment>,
    fanout: usize,
    kdf: KeyDerivationFunction,
}

impl LmdbWalletStoreFactory {
    pub fn new(env: Arc<LmdbEnvironment>, fanout: usize, kdf: KeyDerivationFunction) -> Self {
        Self { env, fanout, kdf }
    }

    fn wallet_path(&self, wallet_id: WalletId) -> PathBuf {
        PathBuf::from(wallet_id.to_string())
    }
}

impl WalletStoreFactory for LmdbWalletStoreFactory {
    fn open_existing(&self, wallet_id: WalletId) -> Result<Arc<dyn WalletStore>> {
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
    ) -> Result<Arc<dyn WalletStore>> {
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

    fn create_from_json(&self, wallet_id: WalletId, json: &str) -> Result<Arc<dyn WalletStore>> {
        let path = self.wallet_path(wallet_id);
        let store =
            LmdbWalletStore::new_from_json(self.fanout, self.kdf.clone(), &self.env, &path, json)?;
        Ok(Arc::new(store))
    }
}

impl WalletStore for LmdbWalletStore {
    fn password(&self) -> RawKey {
        LmdbWalletStore::password(self)
    }

    fn valid_password(&self, txn: &dyn Transaction) -> bool {
        LmdbWalletStore::valid_password(self, txn)
    }

    fn attempt_password(&self, txn: &dyn Transaction, password: &str) -> bool {
        LmdbWalletStore::attempt_password(self, txn, password)
    }

    fn rekey(&self, txn: &mut WriteTransaction, password: &str) -> Result<()> {
        LmdbWalletStore::rekey(self, txn, password)
    }

    fn lock(&self) {
        LmdbWalletStore::lock(self)
    }

    fn is_open(&self) -> bool {
        LmdbWalletStore::is_open(self)
    }

    fn deterministic_key(&self, txn: &dyn Transaction, index: u32) -> RawKey {
        LmdbWalletStore::deterministic_key(self, txn, index)
    }

    fn deterministic_insert(&self, txn: &mut WriteTransaction) -> PublicKey {
        LmdbWalletStore::deterministic_insert(self, txn)
    }

    fn deterministic_insert_at(&self, txn: &mut WriteTransaction, index: u32) -> PublicKey {
        LmdbWalletStore::deterministic_insert_at(self, txn, index)
    }

    fn deterministic_index_get(&self, txn: &dyn Transaction) -> u32 {
        LmdbWalletStore::deterministic_index_get(self, txn)
    }

    fn insert_adhoc(&self, txn: &mut WriteTransaction, prv: &RawKey) -> PublicKey {
        LmdbWalletStore::insert_adhoc(self, txn, prv)
    }

    fn insert_watch(&self, txn: &mut WriteTransaction, pub_key: &PublicKey) -> Result<()> {
        LmdbWalletStore::insert_watch(self, txn, pub_key)
    }

    fn fetch(&self, txn: &dyn Transaction, pub_key: &PublicKey) -> Result<RawKey> {
        LmdbWalletStore::fetch(self, txn, pub_key)
    }

    fn erase(&self, txn: &mut WriteTransaction, pub_key: &PublicKey) {
        LmdbWalletStore::erase(self, txn, pub_key)
    }

    fn exists(&self, txn: &dyn Transaction, pub_key: &PublicKey) -> bool {
        LmdbWalletStore::exists(self, txn, pub_key)
    }

    fn find(&self, txn: &dyn Transaction, pub_key: &PublicKey) -> Option<WalletValue> {
        LmdbWalletStore::find(self, txn, pub_key)
    }

    fn get_key_type(&self, txn: &dyn Transaction, pub_key: &PublicKey) -> KeyType {
        LmdbWalletStore::get_key_type(self, txn, pub_key)
    }

    fn representative(&self, txn: &dyn Transaction) -> PublicKey {
        LmdbWalletStore::representative(self, txn)
    }

    fn representative_set(&self, txn: &mut WriteTransaction, representative: &PublicKey) {
        LmdbWalletStore::representative_set(self, txn, representative)
    }

    fn work_get(&self, txn: &dyn Transaction, pub_key: &PublicKey) -> Result<WorkNonce> {
        LmdbWalletStore::work_get(self, txn, pub_key)
    }

    fn work_put(&self, txn: &mut WriteTransaction, pub_key: &PublicKey, work: WorkNonce) {
        LmdbWalletStore::work_put(self, txn, pub_key, work);
    }

    fn seed(&self, txn: &dyn Transaction) -> RawKey {
        LmdbWalletStore::seed(self, txn)
    }

    fn set_seed(&self, txn: &mut WriteTransaction, seed: &RawKey) {
        LmdbWalletStore::set_seed(self, txn, seed)
    }

    fn serialize_json(&self, txn: &dyn Transaction) -> String {
        LmdbWalletStore::serialize_json(self, txn)
    }

    fn write_backup(&self, txn: &dyn Transaction, path: &Path) -> Result<()> {
        LmdbWalletStore::write_backup(self, txn, path)
    }

    fn iter<'a>(&'a self, txn: &'a dyn Transaction) -> WalletStoreIterator<'a> {
        Box::new(LmdbWalletStore::iter(self, txn))
    }

    fn destroy(&self, txn: &mut WriteTransaction) {
        LmdbWalletStore::destroy(self, txn)
    }
}
