use std::{
    io::{Read, Write},
    path::Path,
    sync::Arc,
};

use anyhow::Result;
use num_derive::FromPrimitive;
use rsnano_nullable_lmdb::{Transaction, WriteTransaction};
use rsnano_types::{DeserializationError, PublicKey, RawKey, WalletId, WorkNonce, read_u64_ne};

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
    fn deterministic_index_set(&self, txn: &mut WriteTransaction, index: u32);
    fn deterministic_clear(&self, txn: &mut WriteTransaction);
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

    fn move_keys(
        &self,
        txn: &mut WriteTransaction,
        other: &dyn WalletStore,
        keys: &[PublicKey],
    ) -> Result<()> {
        assert!(self.valid_password(txn));
        assert!(other.valid_password(txn));

        for key in keys {
            let prv = other.fetch(txn, key)?;
            self.insert_adhoc(txn, &prv);
            other.erase(txn, key);
        }

        Ok(())
    }

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
    fn list_wallet_ids(&self) -> Result<Vec<WalletId>>;
}

pub struct WalletValue {
    pub key: RawKey,
    pub work: WorkNonce,
}

impl WalletValue {
    pub const SERIALIZED_SIZE: usize = RawKey::SERIALIZED_SIZE + 8;

    pub fn new(key: RawKey, work: WorkNonce) -> Self {
        Self { key, work }
    }

    pub fn to_bytes(&self) -> [u8; Self::SERIALIZED_SIZE] {
        let mut buffer = [0; Self::SERIALIZED_SIZE];
        self.serialize(&mut buffer.as_mut()).unwrap();
        buffer
    }

    pub fn serialize<T>(&self, writer: &mut T) -> std::io::Result<()>
    where
        T: Write,
    {
        writer.write_all(self.key.as_bytes())?;
        writer.write_all(&u64::from(self.work).to_ne_bytes())
    }

    pub fn deserialize<T>(reader: &mut T) -> Result<Self, DeserializationError>
    where
        T: Read,
    {
        let key = RawKey::deserialize(reader)?;
        let work = read_u64_ne(reader)?;
        Ok(WalletValue::new(key, work.into()))
    }
}

#[derive(FromPrimitive, Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyType {
    NotAType,
    Unknown,
    Adhoc,
    Deterministic,
}
