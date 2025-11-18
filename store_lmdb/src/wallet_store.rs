use std::{
    fs::{File, Permissions, set_permissions},
    io::Write,
    ops::RangeBounds,
    os::unix::prelude::PermissionsExt,
    path::Path,
    sync::{Mutex, MutexGuard},
};

use anyhow::bail;

use rsnano_nullable_lmdb::{DatabaseFlags, LmdbEnvironment, WriteFlags};
use rsnano_types::{
    Account, KeyDerivationFunction, PublicKey, RawKey, WorkNonce, deterministic_key,
};
use store_traits::{
    transaction::{WalletReadTxn, WalletWriteTxn},
    types::StoreDatabase,
    wallet::{KeyType, WalletStore, WalletStoreIterator, WalletValue},
};

use crate::{
    Fan, LmdbDatabase, LmdbRangeIterator,
    store_utils::{lmdb_ro_cursor_from_store, store_database_from_lmdb, store_write_flags_from},
    transaction::LmdbLedgerWriteTxn,
};

pub struct Fans {
    pub password: Fan,
    pub wallet_key_mem: Fan,
}

impl Fans {
    pub fn new(fanout: usize) -> Self {
        Self {
            password: Fan::new(RawKey::ZERO, fanout),
            wallet_key_mem: Fan::new(RawKey::ZERO, fanout),
        }
    }
}

pub struct LmdbWalletStore {
    db_handle: Mutex<Option<LmdbDatabase>>,
    fans: Mutex<Fans>,
    kdf: KeyDerivationFunction,
}

impl LmdbWalletStore {
    pub const VERSION_CURRENT: u32 = 4;

    pub fn new(
        fanout: usize,
        kdf: KeyDerivationFunction,
        env: &LmdbEnvironment,
        representative: &PublicKey,
        wallet: &Path,
    ) -> anyhow::Result<Self> {
        let store = Self {
            db_handle: Mutex::new(None),
            fans: Mutex::new(Fans::new(fanout)),
            kdf,
        };
        store.initialize(env, wallet)?;
        let mut txn = LmdbLedgerWriteTxn::new(env.begin_write());
        let store_db = store.store_db();
        let needs_init = match txn.get(store_db, Self::version_special().as_bytes()) {
            Err(e) if e.is_not_found() => true,
            Err(e) => panic!("unexpected wallet store error: {:?}", e),
            Ok(_) => false,
        };
        if needs_init {
            store.version_put(&mut txn, Self::VERSION_CURRENT);
            let salt = RawKey::random();
            store.entry_put_raw(
                &mut txn,
                &Self::salt_special(),
                &WalletValue::new(salt, 0.into()),
            );
            // Wallet key is a fixed random key that encrypts all entries
            let wallet_key = RawKey::random();
            let password = RawKey::ZERO;
            let mut guard = store.fans.lock().unwrap();
            guard.password.value_set(password);
            let zero = RawKey::ZERO;
            // Wallet key is encrypted by the user's password
            let encrypted = wallet_key.encrypt(&zero, &salt.initialization_vector_low());
            store.entry_put_raw(
                &mut txn,
                &Self::wallet_key_special(),
                &WalletValue::new(encrypted, 0.into()),
            );
            let wallet_key_enc = encrypted;
            guard.wallet_key_mem.value_set(wallet_key_enc);
            drop(guard);
            let check = zero.encrypt(&wallet_key, &salt.initialization_vector_low());
            store.entry_put_raw(
                &mut txn,
                &Self::check_special(),
                &WalletValue::new(check, 0.into()),
            );
            let rep = RawKey::from_bytes(*representative.as_bytes());
            store.entry_put_raw(
                &mut txn,
                &Self::representative_special(),
                &WalletValue::new(rep, 0.into()),
            );
            let seed = RawKey::random();
            store.set_seed(&mut txn, &seed);
            store.entry_put_raw(
                &mut txn,
                &Self::deterministic_index_special(),
                &WalletValue::new(RawKey::ZERO, 0.into()),
            );
        }
        {
            let key = store.entry_get_raw(&txn, &Self::wallet_key_special()).key;
            let mut guard = store.fans.lock().unwrap();
            guard.wallet_key_mem.value_set(key);
        }
        txn.commit().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(store)
    }

    pub fn new_from_json(
        fanout: usize,
        kdf: KeyDerivationFunction,
        env: &LmdbEnvironment,
        wallet: &Path,
        json: &str,
    ) -> anyhow::Result<Self> {
        let store = Self {
            db_handle: Mutex::new(None),
            fans: Mutex::new(Fans::new(fanout)),
            kdf,
        };
        store.initialize(env, wallet)?;
        let mut txn = LmdbLedgerWriteTxn::new(env.begin_write());
        match txn.get(store.store_db(), Self::version_special().as_bytes()) {
            Ok(_) => panic!("wallet store already initialized"),
            Err(e) if e.is_not_found() => {}
            Err(e) => panic!("unexpected wallet store error: {:?}", e),
        }

        let json: serde_json::Value = serde_json::from_str(json)?;
        if let serde_json::Value::Object(map) = json {
            for (k, v) in map.iter() {
                if let serde_json::Value::String(v_str) = v {
                    let key =
                        PublicKey::decode_hex(k).ok_or_else(|| anyhow!("Invalid public key"))?;
                    let value =
                        RawKey::decode_hex(v_str).ok_or_else(|| anyhow!("Invalid raw key"))?;
                    store.entry_put_raw(&mut txn, &key, &WalletValue::new(value, 0.into()));
                } else {
                    bail!("expected string value");
                }
            }
        } else {
            bail!("invalid json")
        }

        store.ensure_key_exists(&txn, &Self::version_special())?;
        store.ensure_key_exists(&txn, &Self::wallet_key_special())?;
        store.ensure_key_exists(&txn, &Self::salt_special())?;
        store.ensure_key_exists(&txn, &Self::check_special())?;
        store.ensure_key_exists(&txn, &Self::representative_special())?;
        let mut guard = store.fans.lock().unwrap();
        guard.password.value_set(RawKey::ZERO);
        let key = store.entry_get_raw(&txn, &Self::wallet_key_special()).key;
        guard.wallet_key_mem.value_set(key);
        txn.commit().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        drop(guard);
        Ok(store)
    }

    pub fn password(&self) -> RawKey {
        self.fans.lock().unwrap().password.value()
    }

    fn ensure_key_exists(&self, txn: &dyn WalletReadTxn, key: &PublicKey) -> anyhow::Result<()> {
        txn.get(self.store_db(), key.as_bytes())?;
        Ok(())
    }

    /// Wallet version number
    pub fn version_special() -> PublicKey {
        PublicKey::from(0)
    }

    /// Random number used to salt private key encryption
    pub fn salt_special() -> PublicKey {
        PublicKey::from(1)
    }

    /// Key used to encrypt wallet keys, encrypted itself by the user password
    pub fn wallet_key_special() -> PublicKey {
        PublicKey::from(2)
    }

    /// Check value used to see if password is valid
    pub fn check_special() -> PublicKey {
        PublicKey::from(3)
    }

    /// Representative account to be used if we open a new account
    pub fn representative_special() -> PublicKey {
        PublicKey::from(4)
    }

    /// Wallet seed for deterministic key generation
    pub fn seed_special() -> PublicKey {
        PublicKey::from(5)
    }

    /// Current key index for deterministic keys
    pub fn deterministic_index_special() -> PublicKey {
        PublicKey::from(6)
    }

    pub fn special_count() -> PublicKey {
        PublicKey::from(7)
    }

    pub fn initialize(&self, env: &LmdbEnvironment, path: &Path) -> anyhow::Result<()> {
        let path_str = path
            .as_os_str()
            .to_str()
            .ok_or_else(|| anyhow!("invalid path"))?;

        let db = env.create_db(Some(path_str), DatabaseFlags::empty())?;
        *self.db_handle.lock().unwrap() = Some(db);
        Ok(())
    }

    fn db_handle(&self) -> LmdbDatabase {
        self.db_handle.lock().unwrap().unwrap().clone()
    }

    fn store_db(&self) -> StoreDatabase {
        store_database_from_lmdb(self.db_handle())
    }

    pub fn entry_get_raw(&self, txn: &dyn WalletReadTxn, pub_key: &PublicKey) -> WalletValue {
        match txn.get(self.store_db(), pub_key.as_bytes()) {
            Ok(bytes) => {
                let mut slice = bytes.as_ref();
                WalletValue::deserialize(&mut slice).expect("Should be a valid wallet value")
            }
            _ => WalletValue::new(RawKey::ZERO, 0.into()),
        }
    }

    pub fn entry_put_raw(
        &self,
        txn: &mut dyn WalletWriteTxn,
        pub_key: &PublicKey,
        entry: &WalletValue,
    ) {
        txn.put(
            self.store_db(),
            pub_key.as_bytes(),
            &entry.to_bytes(),
            store_write_flags_from(WriteFlags::empty()),
        )
        .unwrap();
    }

    pub fn check(&self, txn: &dyn WalletReadTxn) -> RawKey {
        self.entry_get_raw(txn, &Self::check_special()).key
    }

    pub fn salt(&self, txn: &dyn WalletReadTxn) -> RawKey {
        self.entry_get_raw(txn, &Self::salt_special()).key
    }

    pub fn wallet_key(&self, txn: &dyn WalletReadTxn) -> RawKey {
        let guard = self.fans.lock().unwrap();
        self.wallet_key_locked(&guard, txn)
    }

    fn wallet_key_locked(&self, guard: &MutexGuard<Fans>, txn: &dyn WalletReadTxn) -> RawKey {
        let wallet = guard.wallet_key_mem.value();
        let password = guard.password.value();
        let iv = self.salt(txn).initialization_vector_low();
        wallet.decrypt(&password, &iv)
    }

    pub fn seed(&self, txn: &dyn WalletReadTxn) -> RawKey {
        let value = self.entry_get_raw(txn, &Self::seed_special());
        let password = self.wallet_key(txn);
        let iv = self.salt(txn).initialization_vector_high();
        value.key.decrypt(&password, &iv)
    }

    pub fn set_seed(&self, txn: &mut dyn WalletWriteTxn, prv: &RawKey) {
        let password_l = self.wallet_key(txn);
        let iv = self.salt(txn).initialization_vector_high();
        let ciphertext = prv.encrypt(&password_l, &iv);
        self.entry_put_raw(
            txn,
            &Self::seed_special(),
            &WalletValue::new(ciphertext, 0.into()),
        );
        self.deterministic_clear(txn);
    }

    pub fn deterministic_key(&self, txn: &dyn WalletReadTxn, index: u32) -> RawKey {
        debug_assert!(self.valid_password(txn));
        let seed = self.seed(txn);
        deterministic_key(&seed, index)
    }

    pub fn deterministic_index_get(&self, txn: &dyn WalletReadTxn) -> u32 {
        let value = self.entry_get_raw(txn, &Self::deterministic_index_special());
        value.key.number().low_u32()
    }

    pub fn deterministic_index_set(&self, txn: &mut dyn WalletWriteTxn, index: u32) {
        let index = RawKey::from(index as u64);
        let value = WalletValue::new(index, 0.into());
        self.entry_put_raw(txn, &Self::deterministic_index_special(), &value);
    }

    pub fn set_password(&self, password: RawKey) {
        self.fans.lock().unwrap().password.value_set(password);
    }

    pub fn valid_password(&self, txn: &dyn WalletReadTxn) -> bool {
        let wallet_key = self.wallet_key(txn);
        self.check_wallet_key(txn, &wallet_key)
    }

    pub fn valid_password_locked(&self, guard: &MutexGuard<Fans>, txn: &dyn WalletReadTxn) -> bool {
        let wallet_key = self.wallet_key_locked(guard, txn);
        self.check_wallet_key(txn, &wallet_key)
    }

    fn check_wallet_key(&self, txn: &dyn WalletReadTxn, wallet_key: &RawKey) -> bool {
        let zero = RawKey::ZERO;
        let iv = self.salt(txn).initialization_vector_low();
        let check = zero.encrypt(wallet_key, &iv);
        self.check(txn) == check
    }

    pub fn derive_key(&self, txn: &dyn WalletReadTxn, password: &str) -> RawKey {
        let salt = self.salt(txn);
        self.kdf.hash_password(password, salt.as_bytes())
    }

    pub fn rekey(&self, txn: &mut dyn WalletWriteTxn, password: &str) -> anyhow::Result<()> {
        let mut guard = self.fans.lock().unwrap();
        if self.valid_password_locked(&guard, txn) {
            let password_new = self.derive_key(txn, password);
            let wallet_key = self.wallet_key_locked(&guard, txn);
            guard.password.value_set(password_new);
            let iv = self.salt(txn).initialization_vector_low();
            let encrypted = wallet_key.encrypt(&password_new, &iv);
            guard.wallet_key_mem.value_set(encrypted);
            self.entry_put_raw(
                txn,
                &Self::wallet_key_special(),
                &WalletValue::new(encrypted, 0.into()),
            );
            Ok(())
        } else {
            Err(anyhow!("invalid password"))
        }
    }

    pub fn iter<'tx>(
        &self,
        tx: &'tx dyn WalletReadTxn,
    ) -> impl Iterator<Item = (PublicKey, WalletValue)> + use<'tx> {
        self.iter_range(tx, Self::special_count()..)
    }

    pub fn iter_range<'txn, R>(
        &self,
        tx: &'txn dyn WalletReadTxn,
        range: R,
    ) -> impl Iterator<Item = (PublicKey, WalletValue)> + use<'txn, R>
    where
        R: RangeBounds<PublicKey> + 'static,
    {
        let cursor = tx.open_ro_cursor(self.store_db()).unwrap();
        let cursor = lmdb_ro_cursor_from_store(cursor);
        LmdbRangeIterator::new(
            cursor,
            range.start_bound().map(|b| b.as_bytes().to_vec()),
            range.end_bound().map(|b| b.as_bytes().to_vec()),
            read_wallet_record,
        )
    }

    pub fn find<'txn>(
        &self,
        txn: &'txn dyn WalletReadTxn,
        pub_key: &PublicKey,
    ) -> Option<WalletValue> {
        let mut result = self.iter_range(txn, *pub_key..);
        if let Some((key, value)) = result.next() {
            if key == *pub_key {
                return Some(value);
            }
        }

        None
    }

    pub fn erase(&self, txn: &mut dyn WalletWriteTxn, pub_key: &PublicKey) {
        txn.delete(self.store_db(), pub_key.as_bytes(), None)
            .unwrap();
    }

    pub fn get_key_type(&self, txn: &dyn WalletReadTxn, pub_key: &PublicKey) -> KeyType {
        let value = self.entry_get_raw(txn, pub_key);
        Self::key_type(&value)
    }

    pub fn key_type(value: &WalletValue) -> KeyType {
        let number = value.key.number();
        if number > u64::MAX.into() {
            KeyType::Adhoc
        } else if (number >> 32).low_u32() == 1 {
            KeyType::Deterministic
        } else {
            KeyType::Unknown
        }
    }

    pub fn deterministic_clear(&self, txn: &mut dyn WalletWriteTxn) {
        {
            let mut it = self.iter_range(txn, PublicKey::ZERO..);
            while let Some((account, value)) = it.next() {
                match Self::key_type(&value) {
                    KeyType::Deterministic => {
                        drop(it);
                        self.erase(txn, &account);
                        it = self.iter_range(txn, account..);
                    }
                    _ => {}
                }
            }
        }

        self.deterministic_index_set(txn, 0);
    }

    pub fn valid_public_key(&self, key: &PublicKey) -> bool {
        key.number() >= Self::special_count().number()
    }

    pub fn exists(&self, txn: &dyn WalletReadTxn, key: &PublicKey) -> bool {
        self.valid_public_key(key) && self.find(txn, key).is_some()
    }

    pub fn deterministic_insert(&self, txn: &mut dyn WalletWriteTxn) -> PublicKey {
        let mut index = self.deterministic_index_get(txn);
        let mut prv = self.deterministic_key(txn, index);
        let mut result = PublicKey::from(prv);
        while self.exists(txn, &result) {
            index += 1;
            prv = self.deterministic_key(txn, index);
            result = PublicKey::from(prv);
        }

        let mut marker = 1u64;
        marker <<= 32;
        marker |= index as u64;
        self.entry_put_raw(txn, &result, &WalletValue::new(marker.into(), 0.into()));
        index += 1;
        self.deterministic_index_set(txn, index);
        result
    }

    pub fn deterministic_insert_at(&self, txn: &mut dyn WalletWriteTxn, index: u32) -> PublicKey {
        let prv = self.deterministic_key(txn, index);
        let result = PublicKey::from(prv);
        let mut marker = 1u64;
        marker <<= 32;
        marker |= index as u64;
        self.entry_put_raw(txn, &result, &WalletValue::new(marker.into(), 0.into()));
        result
    }

    pub fn version(&self, txn: &dyn WalletReadTxn) -> u32 {
        let value = self.entry_get_raw(txn, &Self::version_special());
        value.key.as_bytes()[31] as u32
    }

    pub fn attempt_password(&self, txn: &dyn WalletReadTxn, password: &str) -> bool {
        let is_valid = {
            let mut guard = self.fans.lock().unwrap();
            let password_key = self.derive_key(txn, password);
            guard.password.value_set(password_key);
            self.valid_password_locked(&guard, txn)
        };

        if is_valid && self.version(txn) != 4 {
            panic!("invalid wallet store version!");
        }

        is_valid
    }

    pub fn lock(&self) {
        self.fans.lock().unwrap().password.value_set(RawKey::ZERO);
    }

    pub fn accounts(&self, txn: &dyn WalletReadTxn) -> Vec<Account> {
        self.iter(txn).map(|(key, _)| key.into()).collect()
    }

    pub fn representative(&self, txn: &dyn WalletReadTxn) -> PublicKey {
        let value = self.entry_get_raw(txn, &Self::representative_special());
        PublicKey::from_bytes(*value.key.as_bytes())
    }

    pub fn representative_set(&self, txn: &mut dyn WalletWriteTxn, representative: &PublicKey) {
        let rep = RawKey::from_bytes(*representative.as_bytes());
        self.entry_put_raw(
            txn,
            &Self::representative_special(),
            &WalletValue::new(rep, 0.into()),
        );
    }

    pub fn insert_adhoc(&self, txn: &mut dyn WalletWriteTxn, prv: &RawKey) -> PublicKey {
        debug_assert!(self.valid_password(txn));
        let pub_key = PublicKey::from(*prv);
        let password = self.wallet_key(txn);
        let ciphertext = prv.encrypt(&password, &pub_key.initialization_vector());
        self.entry_put_raw(txn, &pub_key, &WalletValue::new(ciphertext, 0.into()));
        pub_key
    }

    pub fn insert_watch(
        &self,
        txn: &mut dyn WalletWriteTxn,
        pub_key: &PublicKey,
    ) -> anyhow::Result<()> {
        if !self.valid_public_key(pub_key) {
            bail!("invalid public key");
        }

        self.entry_put_raw(txn, pub_key, &WalletValue::new(RawKey::ZERO, 0.into()));
        Ok(())
    }

    pub fn fetch(&self, txn: &dyn WalletReadTxn, pub_key: &PublicKey) -> anyhow::Result<RawKey> {
        if !self.valid_password(txn) {
            bail!("invalid password");
        }

        let value = self.entry_get_raw(txn, pub_key);
        if value.key.is_zero() {
            bail!("pub key not found");
        }

        let prv = match Self::key_type(&value) {
            KeyType::Deterministic => {
                let index = value.key.number().low_u32();
                self.deterministic_key(txn, index)
            }
            KeyType::Adhoc => {
                // Ad-hoc keys
                let password = self.wallet_key(txn);
                value
                    .key
                    .decrypt(&password, &pub_key.initialization_vector())
            }
            _ => bail!("invalid key type"),
        };

        let compare = PublicKey::from(prv);
        if compare != *pub_key {
            bail!("expected pub key does not match");
        }
        Ok(prv)
    }

    pub fn serialize_json(&self, tx: &dyn WalletReadTxn) -> String {
        let mut map = serde_json::Map::new();

        // include special keys...
        for (k, v) in self.iter_range(tx, PublicKey::ZERO..) {
            map.insert(
                k.encode_hex(),
                serde_json::Value::String(v.key.encode_hex()),
            );
        }

        serde_json::Value::Object(map).to_string()
    }

    pub fn write_backup(&self, txn: &dyn WalletReadTxn, path: &Path) -> anyhow::Result<()> {
        let mut file = File::create(path)?;
        set_permissions(path, Permissions::from_mode(0o600))?;
        write!(file, "{}", self.serialize_json(txn))?;
        Ok(())
    }

    pub fn move_keys(
        &self,
        txn: &mut dyn WalletWriteTxn,
        other: &LmdbWalletStore,
        keys: &[PublicKey],
    ) -> anyhow::Result<()> {
        debug_assert!(self.valid_password(txn));
        debug_assert!(other.valid_password(txn));
        for k in keys {
            let prv = other.fetch(txn, k)?;
            self.insert_adhoc(txn, &prv);
            other.erase(txn, k);
        }

        Ok(())
    }

    pub fn import(
        &self,
        txn: &mut dyn WalletWriteTxn,
        other: &LmdbWalletStore,
    ) -> anyhow::Result<()> {
        debug_assert!(self.valid_password(txn));
        debug_assert!(other.valid_password(txn));

        enum KeyType {
            Private((PublicKey, RawKey)),
            WatchOnly(PublicKey),
        }

        let mut keys = Vec::new();
        {
            for (k, _) in other.iter(txn) {
                let prv = other.fetch(txn, &k)?;
                if !prv.is_zero() {
                    keys.push(KeyType::Private((k, prv)));
                } else {
                    keys.push(KeyType::WatchOnly(k));
                }
            }
        }

        for k in keys {
            match k {
                KeyType::Private((pub_key, priv_key)) => {
                    self.insert_adhoc(txn, &priv_key);
                    other.erase(txn, &pub_key);
                }
                KeyType::WatchOnly(pub_key) => {
                    self.insert_watch(txn, &pub_key).unwrap();
                    other.erase(txn, &pub_key);
                }
            }
        }

        Ok(())
    }

    pub fn work_get(
        &self,
        txn: &dyn WalletReadTxn,
        pub_key: &PublicKey,
    ) -> anyhow::Result<WorkNonce> {
        let entry = self.entry_get_raw(txn, pub_key);
        if !entry.key.is_zero() {
            Ok(entry.work.into())
        } else {
            Err(anyhow!("not found"))
        }
    }

    pub fn version_put(&self, txn: &mut dyn WalletWriteTxn, version: u32) {
        let entry = RawKey::from(version as u64);
        self.entry_put_raw(
            txn,
            &Self::version_special(),
            &WalletValue::new(entry, 0.into()),
        );
    }

    pub fn work_put(&self, txn: &mut dyn WalletWriteTxn, pub_key: &PublicKey, work: WorkNonce) {
        let mut entry = self.entry_get_raw(txn, pub_key);
        debug_assert!(!entry.key.is_zero());
        entry.work = work;
        self.entry_put_raw(txn, pub_key, &entry);
    }

    pub fn destroy(&self, txn: &mut dyn WalletWriteTxn) {
        unsafe {
            txn.drop_db(self.store_db()).unwrap();
        }
        *self.db_handle.lock().unwrap() = None;
    }

    pub fn is_open(&self) -> bool {
        self.db_handle.lock().unwrap().is_some()
    }
}

fn read_wallet_record(k: &[u8], mut v: &[u8]) -> (PublicKey, WalletValue) {
    let key = PublicKey::from_slice(k).expect("Should be a valid key");
    let value = WalletValue::deserialize(&mut v).expect("Should be a valid wallet value");
    (key, value)
}

impl WalletStore for LmdbWalletStore {
    fn password(&self) -> RawKey {
        LmdbWalletStore::password(self)
    }

    fn valid_password(&self, txn: &dyn WalletReadTxn) -> bool {
        LmdbWalletStore::valid_password(self, txn)
    }

    fn attempt_password(&self, txn: &dyn WalletReadTxn, password: &str) -> bool {
        LmdbWalletStore::attempt_password(self, txn, password)
    }

    fn rekey(&self, txn: &mut dyn WalletWriteTxn, password: &str) -> anyhow::Result<()> {
        LmdbWalletStore::rekey(self, txn, password)
    }

    fn lock(&self) {
        LmdbWalletStore::lock(self)
    }

    fn is_open(&self) -> bool {
        LmdbWalletStore::is_open(self)
    }

    fn deterministic_key(&self, txn: &dyn WalletReadTxn, index: u32) -> RawKey {
        LmdbWalletStore::deterministic_key(self, txn, index)
    }

    fn deterministic_insert(&self, txn: &mut dyn WalletWriteTxn) -> PublicKey {
        LmdbWalletStore::deterministic_insert(self, txn)
    }

    fn deterministic_insert_at(&self, txn: &mut dyn WalletWriteTxn, index: u32) -> PublicKey {
        LmdbWalletStore::deterministic_insert_at(self, txn, index)
    }

    fn deterministic_index_get(&self, txn: &dyn WalletReadTxn) -> u32 {
        LmdbWalletStore::deterministic_index_get(self, txn)
    }

    fn deterministic_index_set(&self, txn: &mut dyn WalletWriteTxn, index: u32) {
        LmdbWalletStore::deterministic_index_set(self, txn, index)
    }

    fn deterministic_clear(&self, txn: &mut dyn WalletWriteTxn) {
        LmdbWalletStore::deterministic_clear(self, txn)
    }

    fn insert_adhoc(&self, txn: &mut dyn WalletWriteTxn, prv: &RawKey) -> PublicKey {
        LmdbWalletStore::insert_adhoc(self, txn, prv)
    }

    fn insert_watch(
        &self,
        txn: &mut dyn WalletWriteTxn,
        pub_key: &PublicKey,
    ) -> anyhow::Result<()> {
        LmdbWalletStore::insert_watch(self, txn, pub_key)
    }

    fn fetch(&self, txn: &dyn WalletReadTxn, pub_key: &PublicKey) -> anyhow::Result<RawKey> {
        LmdbWalletStore::fetch(self, txn, pub_key)
    }

    fn erase(&self, txn: &mut dyn WalletWriteTxn, pub_key: &PublicKey) {
        LmdbWalletStore::erase(self, txn, pub_key)
    }

    fn exists(&self, txn: &dyn WalletReadTxn, pub_key: &PublicKey) -> bool {
        LmdbWalletStore::exists(self, txn, pub_key)
    }

    fn find(&self, txn: &dyn WalletReadTxn, pub_key: &PublicKey) -> Option<WalletValue> {
        LmdbWalletStore::find(self, txn, pub_key)
    }

    fn get_key_type(&self, txn: &dyn WalletReadTxn, pub_key: &PublicKey) -> KeyType {
        LmdbWalletStore::get_key_type(self, txn, pub_key)
    }

    fn representative(&self, txn: &dyn WalletReadTxn) -> PublicKey {
        LmdbWalletStore::representative(self, txn)
    }

    fn representative_set(&self, txn: &mut dyn WalletWriteTxn, representative: &PublicKey) {
        LmdbWalletStore::representative_set(self, txn, representative)
    }

    fn work_get(&self, txn: &dyn WalletReadTxn, pub_key: &PublicKey) -> anyhow::Result<WorkNonce> {
        LmdbWalletStore::work_get(self, txn, pub_key)
    }

    fn work_put(&self, txn: &mut dyn WalletWriteTxn, pub_key: &PublicKey, work: WorkNonce) {
        LmdbWalletStore::work_put(self, txn, pub_key, work)
    }

    fn seed(&self, txn: &dyn WalletReadTxn) -> RawKey {
        LmdbWalletStore::seed(self, txn)
    }

    fn set_seed(&self, txn: &mut dyn WalletWriteTxn, seed: &RawKey) {
        LmdbWalletStore::set_seed(self, txn, seed)
    }

    fn serialize_json(&self, txn: &dyn WalletReadTxn) -> String {
        LmdbWalletStore::serialize_json(self, txn)
    }

    fn write_backup(&self, txn: &dyn WalletReadTxn, path: &Path) -> anyhow::Result<()> {
        LmdbWalletStore::write_backup(self, txn, path)
    }

    fn iter<'a>(&'a self, txn: &'a dyn WalletReadTxn) -> WalletStoreIterator<'a> {
        Box::new(self.iter(txn))
    }

    fn destroy(&self, txn: &mut dyn WalletWriteTxn) {
        LmdbWalletStore::destroy(self, txn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::LmdbLedgerWriteTxn;
    use rsnano_nullable_lmdb::{
        EnvironmentFlags, EnvironmentOptions, LmdbEnvironment, LmdbEnvironmentFactory,
    };
    use rsnano_types::{DEV_GENESIS_KEY, KeyDerivationFunction, PrivateKey, PublicKey, RawKey};
    use std::{collections::HashSet, fs, path::PathBuf};
    use uuid::Uuid;

    struct TestFixture {
        dir: PathBuf,
        env: LmdbEnvironment,
    }

    impl TestFixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("wallet_store_{}", Uuid::new_v4()));
            fs::create_dir_all(&dir).unwrap();
            let mut file = dir.clone();
            file.push("wallet.ldb");
            let options = EnvironmentOptions {
                max_dbs: 32,
                map_size: 1024 * 1024,
                flags: EnvironmentFlags::NO_SUB_DIR
                    | EnvironmentFlags::NO_TLS
                    | EnvironmentFlags::NO_META_SYNC
                    | EnvironmentFlags::NO_SYNC,
                path: file,
            };
            let env = LmdbEnvironmentFactory::default().create(options).unwrap();
            Self { dir, env }
        }

        fn begin_write_txn(&self) -> LmdbLedgerWriteTxn {
            LmdbLedgerWriteTxn::new(self.env.begin_write())
        }

        fn wallet_path(&self, name: &str) -> PathBuf {
            self.dir.join(name)
        }
    }

    impl Drop for TestFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    const TEST_KDF_WORK: u32 = 8;

    fn default_representative() -> PublicKey {
        DEV_GENESIS_KEY.public_key()
    }

    fn new_wallet(
        fixture: &TestFixture,
        kdf: &KeyDerivationFunction,
        name: &str,
    ) -> LmdbWalletStore {
        let representative = default_representative();
        LmdbWalletStore::new(
            0,
            kdf.clone(),
            &fixture.env,
            &representative,
            &fixture.wallet_path(name),
        )
        .unwrap()
    }

    #[test]
    fn no_special_keys_accounts() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let wallet = new_wallet(&fixture, &kdf, "0");
        let mut txn = fixture.begin_write_txn();
        let key = PrivateKey::from(42);
        assert!(!wallet.exists(&txn, &key.public_key()));
        wallet.insert_adhoc(&mut txn, &key.raw_key());
        assert!(wallet.exists(&txn, &key.public_key()));

        for i in 0..LmdbWalletStore::special_count().number().as_u64() {
            assert!(!wallet.exists(&txn, &i.into()))
        }
    }

    #[test]
    fn no_key() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let wallet = new_wallet(&fixture, &kdf, "0");
        let txn = fixture.begin_write_txn();
        assert!(wallet.fetch(&txn, &PublicKey::from(42)).is_err());
        assert!(wallet.valid_password(&txn));
    }

    #[test]
    fn fetch_locked() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let wallet = new_wallet(&fixture, &kdf, "0");
        let mut txn = fixture.begin_write_txn();
        assert!(wallet.valid_password(&txn));
        let key1 = PrivateKey::from(42);
        wallet.insert_adhoc(&mut txn, &key1.raw_key());
        let key2 = wallet.deterministic_insert(&mut txn);
        assert!(!key2.is_zero());
        wallet.set_password(RawKey::from(1));
        assert!(wallet.fetch(&txn, &key1.public_key()).is_err());
        assert!(wallet.fetch(&txn, &key2).is_err());
    }

    #[test]
    fn retrieval() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let wallet = new_wallet(&fixture, &kdf, "0");
        let mut txn = fixture.begin_write_txn();
        let key1 = PrivateKey::from(42);
        wallet.insert_adhoc(&mut txn, &key1.raw_key());
        let prv1 = wallet.fetch(&txn, &key1.public_key()).unwrap();
        assert_eq!(prv1, key1.raw_key());
        wallet.set_password(RawKey::from(123));
        assert!(wallet.fetch(&txn, &key1.public_key()).is_err());
        assert!(!wallet.valid_password(&txn));
    }

    #[test]
    fn empty_iteration() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let wallet = new_wallet(&fixture, &kdf, "0");
        let txn = fixture.begin_write_txn();
        assert!(wallet.iter(&txn).next().is_none());
    }

    #[test]
    fn one_item_iteration() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let wallet = new_wallet(&fixture, &kdf, "0");
        let mut txn = fixture.begin_write_txn();
        let key1 = PrivateKey::from(42);
        wallet.insert_adhoc(&mut txn, &key1.raw_key());
        for (k, v) in wallet.iter(&txn) {
            assert_eq!(k, key1.public_key());
            let password = wallet.wallet_key(&txn);
            let key = v.key.decrypt(&password, &k.initialization_vector());
            assert_eq!(key, key1.raw_key());
        }
    }

    #[test]
    fn two_item_iteration() {
        let fixture = TestFixture::new();
        let key1 = PrivateKey::new();
        let key2 = PrivateKey::new();
        let mut pubs = HashSet::new();
        let mut prvs = HashSet::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        {
            let wallet = new_wallet(&fixture, &kdf, "0");
            let mut txn = fixture.begin_write_txn();
            wallet.insert_adhoc(&mut txn, &key1.raw_key());
            wallet.insert_adhoc(&mut txn, &key2.raw_key());
            for (k, v) in wallet.iter(&txn) {
                pubs.insert(k);
                let password = wallet.wallet_key(&txn);
                let key = v.key.decrypt(&password, &k.initialization_vector());
                prvs.insert(key);
            }
        }
        assert_eq!(pubs.len(), 2);
        assert_eq!(prvs.len(), 2);
        assert!(pubs.contains(&key1.public_key()));
        assert!(prvs.contains(&key1.raw_key()));
        assert!(pubs.contains(&key2.public_key()));
        assert!(prvs.contains(&key2.raw_key()));
    }

    #[test]
    fn find_none() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let wallet = new_wallet(&fixture, &kdf, "0");
        let txn = fixture.begin_write_txn();
        assert!(wallet.find(&txn, &PublicKey::from(1000)).is_none());
    }

    #[test]
    fn find_existing() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let wallet = new_wallet(&fixture, &kdf, "0");
        let mut txn = fixture.begin_write_txn();
        let key1 = PrivateKey::new();
        assert!(!wallet.exists(&txn, &key1.public_key()));
        wallet.insert_adhoc(&mut txn, &key1.raw_key());
        assert!(wallet.exists(&txn, &key1.public_key()));
        wallet.find(&txn, &key1.public_key()).unwrap();
    }

    #[test]
    fn rekey() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let store = new_wallet(&fixture, &kdf, "0");
        let password = store.password();
        assert!(password.is_zero());
        let mut txn = fixture.begin_write_txn();
        let key1 = PrivateKey::new();
        store.insert_adhoc(&mut txn, &key1.raw_key());
        assert_eq!(
            store.fetch(&txn, &key1.public_key()).unwrap(),
            key1.raw_key()
        );
        store.rekey(&mut txn, "1").unwrap();
        let password = store.password();
        let password1 = store.derive_key(&txn, "1");
        assert_eq!(password1, password);
        let prv2 = store.fetch(&txn, &key1.public_key()).unwrap();
        assert_eq!(prv2, key1.raw_key());
        store.set_password(RawKey::from(2));
        assert!(store.rekey(&mut txn, "2").is_err());
    }

    #[test]
    fn hash_password() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let store = new_wallet(&fixture, &kdf, "0");
        let txn = fixture.begin_write_txn();
        let hash1 = store.derive_key(&txn, "");
        let hash2 = store.derive_key(&txn, "");
        assert_eq!(hash1, hash2);
        let hash3 = store.derive_key(&txn, "a");
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn reopen_default_password() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        {
            let store = new_wallet(&fixture, &kdf, "0");
            let txn = fixture.begin_write_txn();
            assert!(store.valid_password(&txn));
            txn.commit().expect("wallet txn commit failed");
        }
        {
            let store = new_wallet(&fixture, &kdf, "0");
            let txn = fixture.begin_write_txn();
            assert!(store.valid_password(&txn));
        }
        {
            let store = new_wallet(&fixture, &kdf, "0");
            let mut txn = fixture.begin_write_txn();
            store.rekey(&mut txn, "").unwrap();
            assert!(store.valid_password(&txn));
            txn.commit().expect("wallet txn commit failed");
        }
        {
            let store = new_wallet(&fixture, &kdf, "0");
            let txn = fixture.begin_write_txn();
            assert!(!store.valid_password(&txn));
            store.attempt_password(&txn, " ");
            assert!(!store.valid_password(&txn));
            store.attempt_password(&txn, "");
            assert!(store.valid_password(&txn));
        }
    }

    #[test]
    fn representative() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let store = new_wallet(&fixture, &kdf, "0");
        let mut txn = fixture.begin_write_txn();
        assert!(!store.exists(&txn, &store.representative(&txn)));
        assert_eq!(store.representative(&txn), default_representative());
        let key = PrivateKey::new();
        store.representative_set(&mut txn, &key.public_key());
        assert_eq!(store.representative(&txn), key.public_key());
        assert!(!store.exists(&txn, &store.representative(&txn)));
        store.insert_adhoc(&mut txn, &key.raw_key());
        assert!(store.exists(&txn, &store.representative(&txn)));
    }

    #[test]
    fn serialize_json_empty() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let store1 = new_wallet(&fixture, &kdf, "0");
        let serialized = {
            let txn = fixture.begin_write_txn();
            store1.serialize_json(&txn)
        };
        let store2 = LmdbWalletStore::new_from_json(
            0,
            kdf.clone(),
            &fixture.env,
            &fixture.wallet_path("1"),
            &serialized,
        )
        .unwrap();
        let txn = fixture.begin_write_txn();
        let password1 = store1.wallet_key(&txn);
        let password2 = store2.wallet_key(&txn);
        assert_eq!(password1, password2);
        assert_eq!(store1.salt(&txn), store2.salt(&txn));
        assert_eq!(store1.check(&txn), store2.check(&txn));
        assert_eq!(store1.representative(&txn), store2.representative(&txn));
        assert!(store1.iter(&txn).next().is_none());
        assert!(store2.iter(&txn).next().is_none());
    }

    #[test]
    fn serialize_json_one() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let store1 = new_wallet(&fixture, &kdf, "0");
        let key = PrivateKey::new();
        let serialized = {
            let mut txn = fixture.begin_write_txn();
            store1.insert_adhoc(&mut txn, &key.raw_key());
            let json = store1.serialize_json(&txn);
            txn.commit().expect("wallet txn commit failed");
            json
        };

        let store2 = LmdbWalletStore::new_from_json(
            0,
            kdf.clone(),
            &fixture.env,
            &fixture.wallet_path("1"),
            &serialized,
        )
        .unwrap();
        let txn = fixture.begin_write_txn();
        let password1 = store1.wallet_key(&txn);
        let password2 = store2.wallet_key(&txn);
        assert_eq!(password1, password2);
        assert_eq!(store1.salt(&txn), store2.salt(&txn));
        assert_eq!(store1.check(&txn), store2.check(&txn));
        assert_eq!(store1.representative(&txn), store2.representative(&txn));
        assert!(store2.exists(&txn, &key.public_key()));
        let prv = store2.fetch(&txn, &key.public_key()).unwrap();
        assert_eq!(prv, key.raw_key());
    }

    #[test]
    fn serialize_json_password() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let wallet1 = new_wallet(&fixture, &kdf, "0");
        let key = PrivateKey::new();
        let serialized = {
            let mut txn = fixture.begin_write_txn();
            wallet1.rekey(&mut txn, "password").unwrap();
            wallet1.insert_adhoc(&mut txn, &key.raw_key());
            let json = wallet1.serialize_json(&txn);
            txn.commit().expect("wallet txn commit failed");
            json
        };
        let wallet2 = LmdbWalletStore::new_from_json(
            0,
            kdf,
            &fixture.env,
            &fixture.wallet_path("1"),
            &serialized,
        )
        .unwrap();
        let txn = fixture.begin_write_txn();
        assert!(!wallet2.valid_password(&txn));
        assert!(wallet2.attempt_password(&txn, "password"));
        assert!(wallet2.valid_password(&txn));
        let password1 = wallet1.wallet_key(&txn);
        let password2 = wallet2.wallet_key(&txn);
        assert_eq!(password1, password2);
        assert_eq!(wallet1.salt(&txn), wallet2.salt(&txn));
        assert_eq!(wallet1.check(&txn), wallet2.check(&txn));
        assert_eq!(wallet1.representative(&txn), wallet2.representative(&txn));
        assert!(wallet2.exists(&txn, &key.public_key()));
        let prv = wallet2.fetch(&txn, &key.public_key()).unwrap();
        assert_eq!(prv, key.raw_key());
    }

    #[test]
    fn wallet_store_move() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let wallet1 = new_wallet(&fixture, &kdf, "0");
        let key = PrivateKey::new();
        {
            let mut txn = fixture.begin_write_txn();
            wallet1.insert_adhoc(&mut txn, &key.raw_key());
            txn.commit().expect("wallet txn commit failed");
        }
        let wallet2 = new_wallet(&fixture, &kdf, "1");
        let mut txn = fixture.begin_write_txn();
        let key2 = PrivateKey::new();
        wallet2.insert_adhoc(&mut txn, &key2.raw_key());
        assert!(!wallet1.exists(&txn, &key2.public_key()));
        wallet1
            .move_keys(&mut txn, &wallet2, &[key2.public_key()])
            .unwrap();
        assert!(wallet1.exists(&txn, &key2.public_key()));
        assert!(!wallet2.exists(&txn, &key2.public_key()));
    }

    #[test]
    fn deterministic_keys() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let wallet = new_wallet(&fixture, &kdf, "0");
        let mut txn = fixture.begin_write_txn();
        let key1 = wallet.deterministic_key(&txn, 0);
        let key2 = wallet.deterministic_key(&txn, 0);
        assert_eq!(key1, key2);
        let key3 = wallet.deterministic_key(&txn, 1);
        assert_ne!(key1, key3);
        assert_eq!(wallet.deterministic_index_get(&txn), 0);
        wallet.deterministic_index_set(&mut txn, 1);
        assert_eq!(wallet.deterministic_index_get(&txn), 1);
        let key4 = wallet.deterministic_insert(&mut txn);
        let key5 = wallet.fetch(&txn, &key4).unwrap();
        assert_eq!(key5, key3);
        assert_eq!(wallet.deterministic_index_get(&txn), 2);
        wallet.deterministic_index_set(&mut txn, 1);
        assert_eq!(wallet.deterministic_index_get(&txn), 1);
        wallet.erase(&mut txn, &key4);
        assert!(!wallet.exists(&txn, &key4));
        let key8 = wallet.deterministic_insert(&mut txn);
        assert_eq!(key8, key4);
        let key6 = wallet.deterministic_insert(&mut txn);
        let key7 = wallet.fetch(&txn, &key6).unwrap();
        assert_ne!(key7, key5);
        assert_eq!(wallet.deterministic_index_get(&txn), 3);
        let key9 = PrivateKey::new();
        wallet.insert_adhoc(&mut txn, &key9.raw_key());
        assert!(wallet.exists(&txn, &key9.public_key()));
        wallet.deterministic_clear(&mut txn);
        assert_eq!(wallet.deterministic_index_get(&txn), 0);
        assert!(!wallet.exists(&txn, &key4));
        assert!(!wallet.exists(&txn, &key6));
        assert!(!wallet.exists(&txn, &key8));
        assert!(wallet.exists(&txn, &key9.public_key()));
    }

    #[test]
    fn reseed() {
        let fixture = TestFixture::new();
        let kdf = KeyDerivationFunction::new(TEST_KDF_WORK);
        let wallet = new_wallet(&fixture, &kdf, "0");

        let mut txn = fixture.begin_write_txn();
        let seed1 = RawKey::from(1);
        let seed2 = RawKey::from(2);
        wallet.set_seed(&mut txn, &seed1);
        let seed3 = wallet.seed(&txn);
        assert_eq!(seed3, seed1);
        let key1 = wallet.deterministic_insert(&mut txn);
        wallet.set_seed(&mut txn, &seed2);
        assert_eq!(wallet.deterministic_index_get(&txn), 0);
        let seed4 = wallet.seed(&txn);
        assert_eq!(seed4, seed2);
        let key2 = wallet.deterministic_insert(&mut txn);
        assert_ne!(key2, key1);
        wallet.set_seed(&mut txn, &seed1);
        let seed5 = wallet.seed(&txn);
        assert_eq!(seed5, seed1);
        let key3 = wallet.deterministic_insert(&mut txn);
        assert_eq!(key1, key3);
    }
}
