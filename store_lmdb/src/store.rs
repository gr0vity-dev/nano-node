#[cfg(feature = "ledger_snapshots")]
use crate::forks_store::LmdbForksStore;
use crate::{
    LmdbAccountStore, LmdbBlockStore, LmdbConfirmationHeightStore, LmdbFinalVoteStore,
    LmdbOnlineWeightStore, LmdbPeerStore, LmdbPendingStore, LmdbRepWeightStore, LmdbVersionStore,
    successor_store::LmdbSuccessorStore,
};
use rsnano_nullable_lmdb::LmdbEnvironment;
use std::sync::Arc;
use store_traits::ledger::{LedgerCache, MemoryStats};

use crate::transaction::{LmdbLedgerReadTxn, LmdbLedgerWriteTxn};

pub struct LmdbStore {
    pub env: LmdbEnvironment,
    pub cache: Arc<LedgerCache>,
    pub block: LmdbBlockStore,
    pub account: LmdbAccountStore,
    pub pending: LmdbPendingStore,
    pub rep_weight: Arc<LmdbRepWeightStore>,
    pub confirmation_height: LmdbConfirmationHeightStore,
    pub successors: LmdbSuccessorStore,
    // extract these?
    pub final_vote: LmdbFinalVoteStore,
    pub online_weight: LmdbOnlineWeightStore,
    pub peer: LmdbPeerStore,
    pub version: LmdbVersionStore,
    #[cfg(feature = "ledger_snapshots")]
    pub forks: LmdbForksStore,
}

impl LmdbStore {
    pub fn new_null() -> Self {
        Self::new(LmdbEnvironment::new_null()).unwrap()
    }

    pub fn new(env: LmdbEnvironment) -> anyhow::Result<Self> {
        Ok(Self {
            cache: Arc::new(LedgerCache::new()),
            block: LmdbBlockStore::new(&env)?,
            account: LmdbAccountStore::new(&env)?,
            pending: LmdbPendingStore::new(&env)?,
            online_weight: LmdbOnlineWeightStore::new(&env)?,
            rep_weight: Arc::new(LmdbRepWeightStore::new(&env)?),
            peer: LmdbPeerStore::new(&env)?,
            confirmation_height: LmdbConfirmationHeightStore::new(&env)?,
            final_vote: LmdbFinalVoteStore::new(&env)?,
            successors: LmdbSuccessorStore::new(&env)?,
            version: LmdbVersionStore::new(&env)?,
            #[cfg(feature = "ledger_snapshots")]
            forks: LmdbForksStore::new(&env)?,
            env,
        })
    }

    pub fn memory_stats(&self) -> anyhow::Result<MemoryStats> {
        let stats = self.env.stat()?;
        Ok(MemoryStats {
            branch_pages: stats.branch_pages(),
            depth: stats.depth(),
            entries: stats.entries(),
            leaf_pages: stats.leaf_pages(),
            overflow_pages: stats.overflow_pages(),
            page_size: stats.page_size(),
        })
    }

    pub fn begin_read(&self) -> LmdbLedgerReadTxn {
        LmdbLedgerReadTxn::new(self.env.begin_read())
    }

    pub fn begin_write(&self) -> LmdbLedgerWriteTxn {
        LmdbLedgerWriteTxn::new(self.env.begin_write())
    }

    pub fn block(&self) -> &LmdbBlockStore {
        &self.block
    }

    pub fn account(&self) -> &LmdbAccountStore {
        &self.account
    }

    pub fn pending(&self) -> &LmdbPendingStore {
        &self.pending
    }

    pub fn confirmation_height(&self) -> &LmdbConfirmationHeightStore {
        &self.confirmation_height
    }

    pub fn successors(&self) -> &LmdbSuccessorStore {
        &self.successors
    }

    pub fn final_vote(&self) -> &LmdbFinalVoteStore {
        &self.final_vote
    }

    pub fn peer(&self) -> &LmdbPeerStore {
        &self.peer
    }

    pub fn version(&self) -> &LmdbVersionStore {
        &self.version
    }

    pub fn online_weight(&self) -> &LmdbOnlineWeightStore {
        &self.online_weight
    }

    pub fn rep_weight(&self) -> Arc<LmdbRepWeightStore> {
        self.rep_weight.clone()
    }

    #[cfg(feature = "ledger_snapshots")]
    pub fn forks(&self) -> &LmdbForksStore {
        &self.forks
    }

    pub fn cache(&self) -> &LedgerCache {
        &self.cache
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_nullable_lmdb::{EnvironmentFlags, EnvironmentOptions, LmdbEnvironmentFactory};

    #[test]
    fn create_store() -> anyhow::Result<()> {
        let options = EnvironmentOptions {
            max_dbs: 100,
            map_size: 1024,
            flags: EnvironmentFlags::empty(),
            path: "/nulled/store.ldb".into(),
        };
        let env = LmdbEnvironmentFactory::new_null().create(options)?;
        let _ = LmdbStore::new(env)?;
        Ok(())
    }
}
