use std::sync::Arc;

use anyhow::Result;
use rsnano_nullable_lmdb::{
    DatabaseFlags, EnvironmentOptions, LmdbDatabase, LmdbEnvironment, LmdbEnvironmentFactory,
    WriteFlags,
};
use rsnano_types::{BlockHash, KeyDerivationFunction};
use store_traits::{
    WalletReadTxn, WalletWriteTxn, wallet::WalletEnvironment as WalletEnvironmentTrait,
};

use crate::{WalletReadTxnSHIM, WalletWriteTxnSHIM, wallet_factory::LmdbWalletStoreFactory};

pub struct LmdbWalletEnvironment {
    env: Arc<LmdbEnvironment>,
    _wallets_db: LmdbDatabase,
    send_action_ids: LmdbDatabase,
}

impl LmdbWalletEnvironment {
    pub fn new(env: Arc<LmdbEnvironment>) -> Result<Self> {
        let wallets_db = open_or_create_db(&env, None)?;
        let send_action_ids = open_or_create_db(&env, Some("send_action_ids"))?;
        Ok(Self {
            env,
            _wallets_db: wallets_db,
            send_action_ids,
        })
    }

    pub fn env(&self) -> Arc<LmdbEnvironment> {
        Arc::clone(&self.env)
    }

    pub fn new_null() -> Result<Self> {
        let env = Arc::new(LmdbEnvironment::new_null());
        Self::new(env)
    }

    pub fn create_store_factory(
        &self,
        fanout: usize,
        kdf: KeyDerivationFunction,
    ) -> LmdbWalletStoreFactory {
        LmdbWalletStoreFactory::new(self.env.clone(), fanout, kdf)
    }
}

impl WalletEnvironmentTrait for LmdbWalletEnvironment {
    fn begin_read_txn(&self) -> Box<dyn WalletReadTxn> {
        Box::new(WalletReadTxnSHIM::new(self.env.begin_read()))
    }

    fn begin_write_txn(&self) -> Box<dyn WalletWriteTxn> {
        Box::new(WalletWriteTxnSHIM::new(self.env.begin_write()))
    }

    fn sync(&self) -> Result<()> {
        Ok(self.env.sync()?)
    }

    fn ensure_initialized(&self) -> Result<()> {
        Ok(())
    }

    fn get_send_action_hash(&self, txn: &dyn WalletReadTxn, id: &str) -> Result<Option<BlockHash>> {
        match txn.get(self.send_action_ids.into(), id.as_bytes()) {
            Ok(bytes) => Ok(Some(
                BlockHash::from_slice(bytes)
                    .ok_or_else(|| anyhow::anyhow!("invalid block hash"))?,
            )),
            Err(e) if e.is_not_found() => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set_send_action_hash(
        &self,
        txn: &mut dyn WalletWriteTxn,
        id: &str,
        hash: &BlockHash,
    ) -> Result<()> {
        txn.put(
            self.send_action_ids.into(),
            id.as_bytes(),
            hash.as_bytes(),
            WriteFlags::empty().into(),
        )?;
        Ok(())
    }

    fn clear_send_action_hashes(&self) -> Result<()> {
        let mut txn = self.begin_write_txn();
        txn.as_mut().clear_db(self.send_action_ids.into())?;
        txn.commit();
        Ok(())
    }
}

fn open_or_create_db(env: &Arc<LmdbEnvironment>, name: Option<&str>) -> Result<LmdbDatabase> {
    match env.open_db(name) {
        Ok(db) => Ok(db),
        Err(rsnano_nullable_lmdb::Error::BadDbi | rsnano_nullable_lmdb::Error::NotFound) => {
            Ok(env.create_db(name, DatabaseFlags::empty())?)
        }
        Err(e) => Err(e.into()),
    }
}

pub struct LmdbWalletEnvironmentFactory {
    inner: LmdbEnvironmentFactory,
}

impl Default for LmdbWalletEnvironmentFactory {
    fn default() -> Self {
        Self::new(LmdbEnvironmentFactory::default())
    }
}

impl LmdbWalletEnvironmentFactory {
    pub fn new(inner: LmdbEnvironmentFactory) -> Self {
        Self { inner }
    }

    pub fn new_null() -> Self {
        Self::new(LmdbEnvironmentFactory::new_null())
    }

    pub fn create(&self, options: EnvironmentOptions) -> Result<Arc<LmdbWalletEnvironment>> {
        let env = self.inner.create(options)?;
        Ok(Arc::new(LmdbWalletEnvironment::new(Arc::new(env))?))
    }
}
