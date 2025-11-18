use std::{
    ops::Bound,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Result, anyhow};
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::{BlockHash, SavedBlock};
use store_traits::{
    environment::StoreCursor,
    ledger::{BlockStore, RangeBounds, StoreIterator},
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::{StoreDatabase, StoreValue, StoreWriteFlags},
};

use crate::{
    BLOCK_DATA_CF_NAME, BLOCK_INDEX_CF_NAME, RocksdbCursor, RocksdbStoreEnvironment,
    rocksdb_ro_cursor_from_store, transaction::RocksdbLedgerReadTxn, value_in_range,
};

pub struct RocksdbBlockStore {
    index_cf: StoreDatabase,
    data_cf: StoreDatabase,
    put_listener: OutputListenerMt<SavedBlock>,
    next_id: AtomicU64,
}

impl RocksdbBlockStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let index_cf = env.open_db(Some(BLOCK_INDEX_CF_NAME))?;
        let data_cf = env.open_db(Some(BLOCK_DATA_CF_NAME))?;
        let next_id = find_next_block_id(&env, data_cf)?;
        Ok(Self {
            index_cf,
            data_cf,
            put_listener: OutputListenerMt::new(),
            next_id: AtomicU64::new(next_id),
        })
    }

    fn index_cf(&self) -> StoreDatabase {
        self.index_cf
    }

    fn data_cf(&self) -> StoreDatabase {
        self.data_cf
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<SavedBlock>> {
        self.put_listener.track()
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, block: &SavedBlock) {
        if self.put_listener.is_tracked() {
            self.put_listener.emit(block.clone());
        }

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let id_bytes = id.to_be_bytes();

        txn.put(
            self.index_cf(),
            block.hash().as_bytes(),
            &id_bytes,
            StoreWriteFlags::default(),
        )
        .expect("failed to write block index");

        txn.put(
            self.data_cf(),
            &id_bytes,
            &block.serialize_with_sideband(),
            StoreWriteFlags::default(),
        )
        .expect("failed to write block data");
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn, hash: &BlockHash) -> Option<SavedBlock> {
        let id_bytes = match txn.get(self.index_cf(), hash.as_bytes()) {
            Ok(bytes) => bytes,
            Err(e) if e.is_not_found() => return None,
            // TODO(store-errors): propagate backend errors instead of panicking once traits return StoreResult.
            Err(e) => panic!("failed to read block index: {e}"),
        };
        self.load_block_bytes(txn, id_bytes.as_ref())
    }

    pub fn exists(&self, txn: &dyn LedgerReadTxn, hash: &BlockHash) -> bool {
        txn.raw_exists(self.index_cf(), hash.as_bytes())
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, hash: &BlockHash) {
        let id = match txn.get(self.index_cf(), hash.as_bytes()) {
            Ok(bytes) => bytes,
            Err(e) if e.is_not_found() => return,
            // TODO(store-errors): propagate backend errors instead of panicking once traits return StoreResult.
            Err(e) => panic!("failed to delete block: {e}"),
        };
        let id_vec = id.as_ref().to_vec();
        txn.delete(self.data_cf(), &id_vec, None)
            .expect("failed to delete block data");
        txn.delete(self.index_cf(), hash.as_bytes(), None)
            .expect("failed to delete block index");
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.raw_count(self.index_cf())
    }

    pub fn iter<'txn>(&'txn self, txn: &'txn dyn LedgerReadTxn) -> StoreIterator<'txn, SavedBlock> {
        let cursor = txn
            .open_ro_cursor(self.index_cf())
            .expect("failed to open block index cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbBlockIterator::new(cursor, txn, self.data_cf()))
    }

    pub fn iter_range<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
        range: RangeBounds<BlockHash>,
    ) -> StoreIterator<'txn, SavedBlock> {
        let cursor = txn
            .open_ro_cursor(self.index_cf())
            .expect("failed to open block index cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbBlockRangeIterator::new(
            cursor,
            txn,
            self.data_cf(),
            range,
        ))
    }

    fn load_block_bytes(&self, txn: &dyn LedgerReadTxn, id_bytes: &[u8]) -> Option<SavedBlock> {
        match txn.get(self.data_cf(), id_bytes) {
            Ok(data) => {
                let mut slice = data.as_ref();
                Some(SavedBlock::deserialize(&mut slice).expect("failed to deserialize block"))
            }
            Err(e) if e.is_not_found() => None,
            // TODO(store-errors): propagate backend errors instead of panicking once traits return StoreResult.
            Err(e) => panic!("failed to read block data: {e}"),
        }
    }
}

impl BlockStore for RocksdbBlockStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, block: &SavedBlock) {
        RocksdbBlockStore::put(self, txn, block);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, hash: &BlockHash) -> Option<SavedBlock> {
        RocksdbBlockStore::get(self, txn, hash)
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, hash: &BlockHash) {
        RocksdbBlockStore::del(self, txn, hash);
    }

    fn exists(&self, txn: &dyn LedgerReadTxn, hash: &BlockHash) -> bool {
        RocksdbBlockStore::exists(self, txn, hash)
    }

    fn iter<'a>(&'a self, txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, SavedBlock> {
        RocksdbBlockStore::iter(self, txn)
    }

    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
        range: RangeBounds<BlockHash>,
    ) -> StoreIterator<'a, SavedBlock> {
        RocksdbBlockStore::iter_range(self, txn, range)
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<SavedBlock>> {
        RocksdbBlockStore::track_puts(self)
    }
}

struct RocksdbBlockIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
    txn: &'txn dyn LedgerReadTxn,
    data_cf: StoreDatabase,
}

impl<'txn> RocksdbBlockIterator<'txn> {
    fn new(
        cursor: RocksdbCursor<'txn>,
        txn: &'txn dyn LedgerReadTxn,
        data_cf: StoreDatabase,
    ) -> Self {
        Self {
            cursor,
            txn,
            data_cf,
        }
    }
}

impl<'txn> Iterator for RocksdbBlockIterator<'txn> {
    type Item = SavedBlock;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let item = self
                .cursor
                .next()
                .expect("failed to advance RocksDB cursor");
            let (hash_bytes, id_bytes) = match item {
                Some(value) => value,
                None => return None,
            };
            let _hash = BlockHash::from_slice(hash_bytes.as_ref())
                .expect("invalid block hash bytes in RocksDB");
            let block = match self.txn.get(self.data_cf, id_bytes.as_ref()) {
                Ok(data) => {
                    let mut slice = data.as_ref();
                    SavedBlock::deserialize(&mut slice)
                        .expect("failed to deserialize RocksDB block")
                }
                Err(e) if e.is_not_found() => continue,
                // TODO(store-errors): propagate backend errors instead of panicking once traits return StoreResult.
                Err(e) => panic!("failed to load block data: {e}"),
            };
            return Some(block);
        }
    }
}

struct RocksdbBlockRangeIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
    txn: &'txn dyn LedgerReadTxn,
    data_cf: StoreDatabase,
    range: RangeBounds<BlockHash>,
    initialized: bool,
}

impl<'txn> RocksdbBlockRangeIterator<'txn> {
    fn new(
        cursor: RocksdbCursor<'txn>,
        txn: &'txn dyn LedgerReadTxn,
        data_cf: StoreDatabase,
        range: RangeBounds<BlockHash>,
    ) -> Self {
        Self {
            cursor,
            txn,
            data_cf,
            range,
            initialized: false,
        }
    }

    fn seek_start(&mut self) -> store_traits::types::StoreResult<Option<(StoreValue, StoreValue)>> {
        match &self.range.start {
            Bound::Included(hash) => self.cursor.seek_lower_bound(hash.as_bytes()),
            Bound::Excluded(hash) => self.cursor.seek_upper_bound(hash.as_bytes()),
            Bound::Unbounded => self.cursor.next(),
        }
    }

    fn advance_cursor(
        &mut self,
    ) -> store_traits::types::StoreResult<Option<(StoreValue, StoreValue)>> {
        if self.initialized {
            self.cursor.next()
        } else {
            self.initialized = true;
            self.seek_start()
        }
    }
}

impl<'txn> Iterator for RocksdbBlockRangeIterator<'txn> {
    type Item = SavedBlock;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let item = self
                .advance_cursor()
                .expect("failed to advance RocksDB cursor");
            let (hash_bytes, id_bytes) = match item {
                Some(value) => value,
                None => return None,
            };
            let hash = BlockHash::from_slice(hash_bytes.as_ref())
                .expect("invalid block hash bytes in RocksDB");
            if !value_in_range(&hash, &self.range) {
                return None;
            }
            let block = match self.txn.get(self.data_cf, id_bytes.as_ref()) {
                Ok(data) => {
                    let mut slice = data.as_ref();
                    SavedBlock::deserialize(&mut slice)
                        .expect("failed to deserialize RocksDB block")
                }
                Err(e) if e.is_not_found() => continue,
                // TODO(store-errors): propagate backend errors instead of panicking once traits return StoreResult.
                Err(e) => panic!("failed to load block data: {e}"),
            };
            return Some(block);
        }
    }
}

fn find_next_block_id(
    env: &Arc<RocksdbStoreEnvironment>,
    data_cf: StoreDatabase,
) -> anyhow::Result<u64> {
    let txn = RocksdbLedgerReadTxn::new(env);
    let cursor = txn
        .open_ro_cursor(data_cf)
        .map_err(|e| anyhow!(e.to_string()))?;
    let mut cursor = rocksdb_ro_cursor_from_store(cursor);
    let mut max_id: Option<u64> = None;
    loop {
        match cursor.next() {
            Ok(Some((key, _))) => {
                let id = u64::from_be_bytes(
                    key.as_ref()
                        .try_into()
                        .map_err(|_| anyhow!("invalid block id bytes"))?,
                );
                max_id = Some(max_id.map_or(id, |current| current.max(id)));
            }
            Ok(None) => break,
            Err(e) => return Err(anyhow!(e.to_string())),
        }
    }
    Ok(max_id.map_or(0, |v| v + 1))
}
