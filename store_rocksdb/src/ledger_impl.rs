use std::sync::{Arc, OnceLock};

use anyhow::anyhow;
use rsnano_types::{Account, AccountInfo, ConfirmationHeightInfo};
use store_traits::{
    ledger::{
        AccountStore, BlockStore, ConfirmationHeightStore, FinalVoteStore, LedgerCache,
        LedgerStore, MemoryStats, OnlineWeightStore, PeerStore, PendingStore, RepWeightStore,
        StoreVendor, SuccessorStore, VersionStore, WriteQueueStats, WriteStrategy, WriterType,
    },
    transaction::{LedgerReadTxn, LedgerWriteTxn},
};

use crate::{
    account_store::RocksdbAccountStore,
    block_store::RocksdbBlockStore,
    confirmation_height_store::RocksdbConfirmationHeightStore,
    environment::RocksdbStoreEnvironment,
    final_vote_store::RocksdbFinalVoteStore,
    online_weight_store::RocksdbOnlineWeightStore,
    peer_store::RocksdbPeerStore,
    pending_store::RocksdbPendingStore,
    rep_weight_store::RocksdbRepWeightStore,
    successor_store::RocksdbSuccessorStore,
    transaction::{RocksdbLedgerReadTxn, RocksdbLedgerWriteTxn},
    version_store::RocksdbVersionStore,
};

pub struct RocksdbLedgerStore {
    env: Arc<RocksdbStoreEnvironment>,
    cache: Arc<LedgerCache>,
    block: RocksdbBlockStore,
    account: RocksdbAccountStore,
    pending: RocksdbPendingStore,
    confirmation_height: RocksdbConfirmationHeightStore,
    rep_weight: Arc<RocksdbRepWeightStore>,
    successors: RocksdbSuccessorStore,
    final_vote: RocksdbFinalVoteStore,
    peer: RocksdbPeerStore,
    version: RocksdbVersionStore,
    online_weight: RocksdbOnlineWeightStore,
}

impl RocksdbLedgerStore {
    pub fn create(
        env: Arc<RocksdbStoreEnvironment>,
        cache: Arc<LedgerCache>,
    ) -> anyhow::Result<Arc<dyn LedgerStore>> {
        let block = RocksdbBlockStore::new(Arc::clone(&env))?;
        let account = RocksdbAccountStore::new(Arc::clone(&env))?;
        let pending = RocksdbPendingStore::new(Arc::clone(&env))?;
        let confirmation_height = RocksdbConfirmationHeightStore::new(Arc::clone(&env))?;
        let rep_weight = Arc::new(RocksdbRepWeightStore::new(Arc::clone(&env))?);
        let successors = RocksdbSuccessorStore::new(Arc::clone(&env))?;
        let online_weight = RocksdbOnlineWeightStore::new(Arc::clone(&env))?;
        let final_vote = RocksdbFinalVoteStore::new(Arc::clone(&env))?;
        let peer = RocksdbPeerStore::new(Arc::clone(&env))?;
        let version = RocksdbVersionStore::new(Arc::clone(&env))?;

        Ok(Arc::new(Self {
            env,
            cache,
            block,
            account,
            pending,
            confirmation_height,
            rep_weight,
            successors,
            final_vote,
            peer,
            version,
            online_weight,
        }))
    }
}

impl LedgerStore for RocksdbLedgerStore {
    fn block_store(&self) -> &dyn BlockStore {
        &self.block
    }

    fn account_store(&self) -> &dyn AccountStore {
        &self.account
    }

    fn pending_store(&self) -> &dyn PendingStore {
        &self.pending
    }

    fn confirmation_height_store(&self) -> &dyn ConfirmationHeightStore {
        &self.confirmation_height
    }

    fn successor_store(&self) -> &dyn SuccessorStore {
        &self.successors
    }

    fn final_vote_store(&self) -> &dyn FinalVoteStore {
        &self.final_vote
    }

    fn peer_store(&self) -> &dyn PeerStore {
        &self.peer
    }

    fn version_store(&self) -> &dyn VersionStore {
        &self.version
    }

    fn online_weight_store(&self) -> &dyn OnlineWeightStore {
        &self.online_weight
    }

    fn rep_weight_store(&self) -> Arc<dyn RepWeightStore> {
        self.rep_weight.clone()
    }

    fn begin_read(&self) -> Box<dyn LedgerReadTxn> {
        Box::new(RocksdbLedgerReadTxn::new(&self.env))
    }

    fn begin_write_with_writer(
        &self,
        writer: WriterType,
        strategy: WriteStrategy,
    ) -> Box<dyn LedgerWriteTxn> {
        Box::new(RocksdbLedgerWriteTxn::new_with_writer(
            &self.env, writer, strategy,
        ))
    }

    fn sync(&self) -> anyhow::Result<()> {
        self.env.sync().map_err(|e| anyhow!(e.to_string()))
    }

    fn cache(&self) -> &LedgerCache {
        &self.cache
    }

    fn memory_stats(&self) -> anyhow::Result<MemoryStats> {
        Ok(MemoryStats {
            branch_pages: 0,
            depth: 0,
            entries: 0,
            leaf_pages: 0,
            overflow_pages: 0,
            page_size: 0,
        })
    }

    fn for_each_account_par(
        &self,
        _thread_count: usize,
        action: &(dyn Fn(&mut dyn Iterator<Item = (Account, AccountInfo)>) + Send + Sync),
    ) {
        let txn = RocksdbLedgerReadTxn::new(&self.env);
        let mut iter = self.account.iter(&txn);
        action(&mut iter);
    }

    fn for_each_confirmation_height_par(
        &self,
        _thread_count: usize,
        action: &(
             dyn Fn(&mut dyn Iterator<Item = (Account, ConfirmationHeightInfo)>) + Send + Sync
         ),
    ) {
        let txn = RocksdbLedgerReadTxn::new(&self.env);
        let mut iter = self.confirmation_height.iter(&txn);
        action(&mut iter);
    }

    fn vendor(&self) -> StoreVendor {
        rocksdb_vendor()
    }

    fn write_queue_stats(&self) -> Option<WriteQueueStats> {
        Some(self.env.inner().write_queue().stats())
    }
}

pub fn rocksdb_vendor() -> StoreVendor {
    static VENDOR: OnceLock<StoreVendor> = OnceLock::new();
    VENDOR
        .get_or_init(|| {
            let version = option_env!("RSN_ROCKSDB_LIB_VERSION").unwrap_or("unknown");
            StoreVendor::new("rocksdb", version)
        })
        .clone()
}
