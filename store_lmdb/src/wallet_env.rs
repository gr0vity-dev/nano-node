use std::sync::Arc;

use anyhow::Result;
use rsnano_nullable_lmdb::{DatabaseFlags, LmdbDatabase, LmdbEnvironment, WriteFlags};
use rsnano_types::BlockHash;
use store_traits::wallet::WalletEnvironment as WalletEnvironmentTrait;
use store_traits::{WalletReadTxn, WalletWriteTxn};

use crate::{
    WalletReadTxnSHIM, WalletWriteTxnSHIM,
    wallet_txn_shim::{wallet_lmdb_read_txn, wallet_lmdb_write_txn},
};

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
        let lmdb_txn = wallet_lmdb_read_txn(txn);
        match lmdb_txn.get(self.send_action_ids, id.as_bytes()) {
            Ok(bytes) => Ok(Some(
                BlockHash::from_slice(bytes)
                    .ok_or_else(|| anyhow::anyhow!("invalid block hash"))?,
            )),
            Err(rsnano_nullable_lmdb::Error::NotFound) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set_send_action_hash(
        &self,
        txn: &mut dyn WalletWriteTxn,
        id: &str,
        hash: &BlockHash,
    ) -> Result<()> {
        let lmdb_txn = wallet_lmdb_write_txn(txn);
        lmdb_txn.put(
            self.send_action_ids,
            id.as_bytes(),
            hash.as_bytes(),
            WriteFlags::empty(),
        )?;
        Ok(())
    }

    fn clear_send_action_hashes(&self) -> Result<()> {
        let mut txn = self.begin_write_txn();
        {
            let lmdb_txn = wallet_lmdb_write_txn(txn.as_mut());
            lmdb_txn.clear_db(self.send_action_ids)?;
        }
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
