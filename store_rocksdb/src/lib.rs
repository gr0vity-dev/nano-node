use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    fs,
    io::Cursor,
    iter,
    marker::PhantomData,
    mem,
    net::SocketAddrV6,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

use anyhow::{Result, anyhow, bail};
use parking_lot::RwLock;
use rocksdb::{
    BoundColumnFamily, ColumnFamilyDescriptor, DBIteratorWithThreadMode, DBWithThreadMode,
    Error as RocksError, IteratorMode, MultiThreaded, Options, SnapshotWithThreadMode, WriteBatch,
};
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::{
    Account, AccountInfo, Amount, BlockHash, ConfirmationHeightInfo, PendingInfo, PendingKey,
    PublicKey, QualifiedRoot, SavedBlock,
};
use store_traits::config::{LedgerBackend, LedgerStoreConfig, RocksDbConfig};
use store_traits::environment::{
    StoreCursor, StoreEnvironment, StoreEnvironmentFactory, StoreEnvironmentOptions, StoreReadTxn,
    StoreWriteTxn,
};
use store_traits::ledger::{
    AccountStore, BlockStore, ConfirmationHeightStore, FinalVoteStore, LedgerCache, LedgerStore,
    LedgerStoreFactory, MemoryStats, OnlineWeightStore, PendingStore, PeerStore, RangeBounds,
    RepWeightStore, StoreIterator, SuccessorStore, VersionStore,
};
use store_traits::transaction::{LedgerReadTxn, LedgerWriteTxn};
use store_traits::types::{
    StoreDatabase, StoreEnvironmentFlags, StoreError, StoreErrorKind, StoreResult, StoreRoCursor,
    StoreRwCursor, StoreWriteFlags,
};
pub struct RocksdbStoreEnvironment {
    inner: Arc<RocksDbInner>,
    _temp_dir: Option<tempfile::TempDir>,
}

impl RocksdbStoreEnvironment {
    fn open(
        path: PathBuf,
        _flags: StoreEnvironmentFlags,
        temp_dir: Option<tempfile::TempDir>,
        config: Option<&RocksDbConfig>,
    ) -> anyhow::Result<Self> {
        let inner = RocksDbInner::open(&path, config.and_then(|c| c.max_open_files))?;
        Ok(Self {
            inner: Arc::new(inner),
            _temp_dir: temp_dir,
        })
    }

    fn inner(&self) -> Arc<RocksDbInner> {
        Arc::clone(&self.inner)
    }
}

impl StoreEnvironment for RocksdbStoreEnvironment {
    type ReadTxn<'env>
        = RocksdbReadTxn<'env>
    where
        Self: 'env;
    type WriteTxn<'env>
        = RocksdbWriteTxn<'env>
    where
        Self: 'env;

    fn begin_read(&self) -> Self::ReadTxn<'_> {
        RocksdbReadTxn::new(&self.inner)
    }

    fn begin_write(&self) -> Self::WriteTxn<'_> {
        RocksdbWriteTxn::new(&self.inner)
    }

    fn open_db(&self, name: Option<&str>) -> StoreResult<StoreDatabase> {
        self.inner.open_database(name)
    }

    fn sync(&self) -> StoreResult<()> {
        self.inner.flush_wal()
    }
}

impl LedgerReadTxn for RocksdbLedgerReadTxn {
    fn is_refresh_needed(&self) -> bool {
        false
    }

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        self.inner.get(database, key)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        let cursor = self.inner.open_cursor(database)?;
        Ok(store_ro_cursor_from_rocksdb(cursor))
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        self.inner.count(database)
    }
}

impl LedgerReadTxn for RocksdbLedgerWriteTxn {
    fn is_refresh_needed(&self) -> bool {
        false
    }

    fn get(&self, database: StoreDatabase, key: &[u8]) -> StoreResult<&[u8]> {
        self.inner.get(database, key)
    }

    fn open_ro_cursor(&self, database: StoreDatabase) -> StoreResult<StoreRoCursor<'_>> {
        let cursor = self.inner.open_cursor(database)?;
        Ok(store_ro_cursor_from_rocksdb(cursor))
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        self.inner.count(database)
    }
}

impl LedgerWriteTxn for RocksdbLedgerWriteTxn {
    fn put(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: &[u8],
        flags: StoreWriteFlags,
    ) -> StoreResult<()> {
        self.inner.put(database, key, value, flags)
    }

    fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> StoreResult<()> {
        self.inner.delete(database, key, value)
    }

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        self.inner.clear_db(database)
    }

    fn open_rw_cursor(&mut self, database: StoreDatabase) -> StoreResult<StoreRwCursor<'_>> {
        let cursor = self.inner.open_rw_cursor(database)?;
        Ok(store_rw_cursor_from_rocksdb(cursor))
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        unsafe { self.inner.drop_db(database) }
    }

    fn commit(self: Box<Self>) {
        self.inner.commit();
    }
}

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
            Err(e) => panic!("failed to read block index: {e}"),
        };
        self.load_block_bytes(txn, id_bytes)
    }

    pub fn exists(&self, txn: &dyn LedgerReadTxn, hash: &BlockHash) -> bool {
        txn.raw_exists(self.index_cf(), hash.as_bytes())
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, hash: &BlockHash) {
        let id = match txn.get(self.index_cf(), hash.as_bytes()) {
            Ok(bytes) => bytes,
            Err(e) if e.is_not_found() => return,
            Err(e) => panic!("failed to delete block: {e}"),
        };
        let id_vec = id.to_vec();
        txn.delete(self.data_cf(), &id_vec, None)
            .expect("failed to delete block data");
        txn.delete(self.index_cf(), hash.as_bytes(), None)
            .expect("failed to delete block index");
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.count(self.index_cf())
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
                let mut reader = Cursor::new(data.to_vec());
                Some(SavedBlock::deserialize(&mut reader).expect("failed to deserialize block"))
            }
            Err(e) if e.is_not_found() => None,
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
            let _hash =
                BlockHash::from_slice(hash_bytes).expect("invalid block hash bytes in RocksDB");
            let block = match self.txn.get(self.data_cf, id_bytes) {
                Ok(data) => {
                    let mut reader = Cursor::new(data.to_vec());
                    SavedBlock::deserialize(&mut reader)
                        .expect("failed to deserialize RocksDB block")
                }
                Err(e) if e.is_not_found() => continue,
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
        }
    }
}

impl<'txn> Iterator for RocksdbBlockRangeIterator<'txn> {
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
            let hash =
                BlockHash::from_slice(hash_bytes).expect("invalid block hash bytes in RocksDB");
            if !value_in_range(&hash, &self.range) {
                continue;
            }
            let block = match self.txn.get(self.data_cf, id_bytes) {
                Ok(data) => {
                    let mut reader = Cursor::new(data.to_vec());
                    SavedBlock::deserialize(&mut reader)
                        .expect("failed to deserialize RocksDB block")
                }
                Err(e) if e.is_not_found() => continue,
                Err(e) => panic!("failed to load block data: {e}"),
            };
            return Some(block);
        }
    }
}

pub struct RocksdbAccountStore {
    database: StoreDatabase,
    put_listener: OutputListenerMt<(Account, AccountInfo)>,
}

impl RocksdbAccountStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let database = env.open_db(Some(ACCOUNTS_CF_NAME))?;
        Ok(Self {
            database,
            put_listener: OutputListenerMt::new(),
        })
    }

    fn database(&self) -> StoreDatabase {
        self.database
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<(Account, AccountInfo)>> {
        self.put_listener.track()
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, account: &Account, info: &AccountInfo) {
        if self.put_listener.is_tracked() {
            self.put_listener.emit((*account, info.clone()));
        }

        txn.put(
            self.database(),
            account.as_bytes(),
            &info.to_bytes(),
            StoreWriteFlags::default(),
        )
        .expect("failed to write account info");
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn, account: &Account) -> Option<AccountInfo> {
        match txn.get(self.database(), account.as_bytes()) {
            Ok(mut bytes) => AccountInfo::deserialize(&mut bytes).ok(),
            Err(e) if e.is_not_found() => None,
            Err(e) => panic!("failed to read account info: {e}"),
        }
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, account: &Account) {
        txn.delete(self.database(), account.as_bytes(), None)
            .expect("failed to delete account");
    }

    pub fn iter<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
    ) -> StoreIterator<'txn, (Account, AccountInfo)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open account cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbAccountIterator::new(cursor))
    }

    pub fn iter_range<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
        range: RangeBounds<Account>,
    ) -> StoreIterator<'txn, (Account, AccountInfo)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open account cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbAccountRangeIterator::new(cursor, range))
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.count(self.database())
    }
}

impl AccountStore for RocksdbAccountStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, account: &Account, info: &AccountInfo) {
        RocksdbAccountStore::put(self, txn, account, info);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, account: &Account) -> Option<AccountInfo> {
        RocksdbAccountStore::get(self, txn, account)
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, account: &Account) {
        RocksdbAccountStore::del(self, txn, account);
    }

    fn iter<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
    ) -> StoreIterator<'a, (Account, AccountInfo)> {
        RocksdbAccountStore::iter(self, txn)
    }

    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
        range: RangeBounds<Account>,
    ) -> StoreIterator<'a, (Account, AccountInfo)> {
        RocksdbAccountStore::iter_range(self, txn, range)
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(Account, AccountInfo)>> {
        RocksdbAccountStore::track_puts(self)
    }
}

struct RocksdbAccountIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
}

impl<'txn> RocksdbAccountIterator<'txn> {
    fn new(cursor: RocksdbCursor<'txn>) -> Self {
        Self { cursor }
    }
}

impl<'txn> Iterator for RocksdbAccountIterator<'txn> {
    type Item = (Account, AccountInfo);

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.cursor.next().expect("failed to advance cursor")?;
        Some(read_account_record(entry))
    }
}

struct RocksdbAccountRangeIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
    range: RangeBounds<Account>,
}

impl<'txn> RocksdbAccountRangeIterator<'txn> {
    fn new(cursor: RocksdbCursor<'txn>, range: RangeBounds<Account>) -> Self {
        Self { cursor, range }
    }
}

impl<'txn> Iterator for RocksdbAccountRangeIterator<'txn> {
    type Item = (Account, AccountInfo);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let entry = self.cursor.next().expect("failed to advance cursor")?;
            let record = read_account_record(entry);
            if value_in_range(&record.0, &self.range) {
                return Some(record);
            }
        }
    }
}

fn read_account_record((key, value): (&[u8], &[u8])) -> (Account, AccountInfo) {
    let account = Account::from_bytes(
        key.try_into()
            .expect("invalid account key length in RocksDB"),
    );
    let mut bytes = value;
    let info =
        AccountInfo::deserialize(&mut bytes).expect("failed to deserialize RocksDB account info");
    (account, info)
}

pub struct RocksdbPendingStore {
    database: StoreDatabase,
    put_listener: OutputListenerMt<(PendingKey, PendingInfo)>,
    delete_listener: OutputListenerMt<PendingKey>,
}

impl RocksdbPendingStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let database = env.open_db(Some(PENDING_CF_NAME))?;
        Ok(Self {
            database,
            put_listener: OutputListenerMt::new(),
            delete_listener: OutputListenerMt::new(),
        })
    }

    fn database(&self) -> StoreDatabase {
        self.database
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<(PendingKey, PendingInfo)>> {
        self.put_listener.track()
    }

    pub fn track_deletions(&self) -> Arc<OutputTrackerMt<PendingKey>> {
        self.delete_listener.track()
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey, info: &PendingInfo) {
        if self.put_listener.is_tracked() {
            self.put_listener.emit((key.clone(), info.clone()));
        }

        txn.put(
            self.database(),
            &key.to_bytes(),
            &info.to_bytes(),
            StoreWriteFlags::default(),
        )
        .expect("failed to write pending info");
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey) {
        if self.delete_listener.is_tracked() {
            self.delete_listener.emit(key.clone());
        }

        txn.delete(self.database(), &key.to_bytes(), None)
            .expect("failed to delete pending info");
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn, key: &PendingKey) -> Option<PendingInfo> {
        match txn.get(self.database(), &key.to_bytes()) {
            Ok(mut bytes) => Some(
                PendingInfo::deserialize(&mut bytes)
                    .expect("failed to deserialize RocksDB pending info"),
            ),
            Err(e) if e.is_not_found() => None,
            Err(e) => panic!("failed to read pending info: {e}"),
        }
    }

    pub fn iter<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
    ) -> StoreIterator<'txn, (PendingKey, PendingInfo)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open pending cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbPendingIterator::new(cursor))
    }

    pub fn iter_range<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
        range: RangeBounds<PendingKey>,
    ) -> StoreIterator<'txn, (PendingKey, PendingInfo)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open pending cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbPendingRangeIterator::new(cursor, range))
    }

    pub fn exists(&self, txn: &dyn LedgerReadTxn, key: &PendingKey) -> bool {
        txn.raw_exists(self.database(), &key.to_bytes())
    }

    pub fn any(&self, txn: &dyn LedgerReadTxn, account: &Account) -> bool {
        let start = PendingKey::new(*account, BlockHash::ZERO);
        let range = RangeBounds::new(std::ops::Bound::Included(start), std::ops::Bound::Unbounded);
        self.iter_range(txn, range)
            .next()
            .map(|(key, _)| key.receiving_account == *account)
            .unwrap_or(false)
    }
}

impl PendingStore for RocksdbPendingStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey, pending: &PendingInfo) {
        RocksdbPendingStore::put(self, txn, key, pending);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey) {
        RocksdbPendingStore::del(self, txn, key);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, key: &PendingKey) -> Option<PendingInfo> {
        RocksdbPendingStore::get(self, txn, key)
    }

    fn iter_range<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
        range: RangeBounds<PendingKey>,
    ) -> StoreIterator<'a, (PendingKey, PendingInfo)> {
        RocksdbPendingStore::iter_range(self, txn, range)
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(PendingKey, PendingInfo)>> {
        RocksdbPendingStore::track_puts(self)
    }

    fn track_deletions(&self) -> Arc<OutputTrackerMt<PendingKey>> {
        RocksdbPendingStore::track_deletions(self)
    }
}

struct RocksdbPendingIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
}

impl<'txn> RocksdbPendingIterator<'txn> {
    fn new(cursor: RocksdbCursor<'txn>) -> Self {
        Self { cursor }
    }
}

impl<'txn> Iterator for RocksdbPendingIterator<'txn> {
    type Item = (PendingKey, PendingInfo);

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.cursor.next().expect("failed to advance cursor")?;
        Some(read_pending_record(entry))
    }
}

struct RocksdbPendingRangeIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
    range: RangeBounds<PendingKey>,
}

impl<'txn> RocksdbPendingRangeIterator<'txn> {
    fn new(cursor: RocksdbCursor<'txn>, range: RangeBounds<PendingKey>) -> Self {
        Self { cursor, range }
    }
}

impl<'txn> Iterator for RocksdbPendingRangeIterator<'txn> {
    type Item = (PendingKey, PendingInfo);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let entry = self.cursor.next().expect("failed to advance cursor")?;
            let record = read_pending_record(entry);
            if value_in_range(&record.0, &self.range) {
                return Some(record);
            }
        }
    }
}

fn read_pending_record((key, value): (&[u8], &[u8])) -> (PendingKey, PendingInfo) {
    let mut key_bytes = key;
    let mut value_bytes = value;
    let key =
        PendingKey::deserialize(&mut key_bytes).expect("failed to deserialize RocksDB pending key");
    let info = PendingInfo::deserialize(&mut value_bytes)
        .expect("failed to deserialize RocksDB pending info");
    (key, info)
}

pub struct RocksdbConfirmationHeightStore {
    database: StoreDatabase,
}

impl RocksdbConfirmationHeightStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let database = env.open_db(Some(CONF_HEIGHT_CF_NAME))?;
        Ok(Self { database })
    }

    fn database(&self) -> StoreDatabase {
        self.database
    }

    pub fn put(
        &self,
        txn: &mut dyn LedgerWriteTxn,
        account: &Account,
        info: &ConfirmationHeightInfo,
    ) {
        txn.put(
            self.database(),
            account.as_bytes(),
            &info.to_bytes(),
            StoreWriteFlags::default(),
        )
        .expect("failed to write confirmation height info");
    }

    pub fn get(
        &self,
        txn: &dyn LedgerReadTxn,
        account: &Account,
    ) -> Option<ConfirmationHeightInfo> {
        match txn.get(self.database(), account.as_bytes()) {
            Ok(mut bytes) => ConfirmationHeightInfo::deserialize(&mut bytes).ok(),
            Err(e) if e.is_not_found() => None,
            Err(e) => panic!("failed to read confirmation height: {e}"),
        }
    }

    pub fn exists(&self, txn: &dyn LedgerReadTxn, account: &Account) -> bool {
        txn.raw_exists(self.database(), account.as_bytes())
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, account: &Account) {
        txn.delete(self.database(), account.as_bytes(), None)
            .expect("failed to delete confirmation height");
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.count(self.database())
    }

    pub fn clear(&self, txn: &mut dyn LedgerWriteTxn) {
        txn.clear_db(self.database())
            .expect("failed to clear confirmation height");
    }

    pub fn iter<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
    ) -> StoreIterator<'txn, (Account, ConfirmationHeightInfo)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open confirmation height cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbConfirmationHeightIterator::new(cursor))
    }

    pub fn iter_range<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
        range: RangeBounds<Account>,
    ) -> StoreIterator<'txn, (Account, ConfirmationHeightInfo)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open confirmation height cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbConfirmationHeightRangeIterator::new(cursor, range))
    }
}

impl ConfirmationHeightStore for RocksdbConfirmationHeightStore {
    fn put(
        &self,
        txn: &mut dyn LedgerWriteTxn,
        account: &Account,
        info: &ConfirmationHeightInfo,
    ) {
        RocksdbConfirmationHeightStore::put(self, txn, account, info);
    }

    fn get(
        &self,
        txn: &dyn LedgerReadTxn,
        account: &Account,
    ) -> Option<ConfirmationHeightInfo> {
        RocksdbConfirmationHeightStore::get(self, txn, account)
    }

    fn exists(&self, txn: &dyn LedgerReadTxn, account: &Account) -> bool {
        RocksdbConfirmationHeightStore::exists(self, txn, account)
    }

    fn iter<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
    ) -> StoreIterator<'a, (Account, ConfirmationHeightInfo)> {
        RocksdbConfirmationHeightStore::iter(self, txn)
    }
}

struct RocksdbConfirmationHeightIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
}

impl<'txn> RocksdbConfirmationHeightIterator<'txn> {
    fn new(cursor: RocksdbCursor<'txn>) -> Self {
        Self { cursor }
    }
}

impl<'txn> Iterator for RocksdbConfirmationHeightIterator<'txn> {
    type Item = (Account, ConfirmationHeightInfo);

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.cursor.next().expect("failed to advance cursor")?;
        Some(read_confirmation_height_record(entry))
    }
}

struct RocksdbConfirmationHeightRangeIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
    range: RangeBounds<Account>,
}

impl<'txn> RocksdbConfirmationHeightRangeIterator<'txn> {
    fn new(cursor: RocksdbCursor<'txn>, range: RangeBounds<Account>) -> Self {
        Self { cursor, range }
    }
}

impl<'txn> Iterator for RocksdbConfirmationHeightRangeIterator<'txn> {
    type Item = (Account, ConfirmationHeightInfo);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let entry = self.cursor.next().expect("failed to advance cursor")?;
            let record = read_confirmation_height_record(entry);
            if value_in_range(&record.0, &self.range) {
                return Some(record);
            }
        }
    }
}

fn read_confirmation_height_record(
    (key, value): (&[u8], &[u8]),
) -> (Account, ConfirmationHeightInfo) {
    let account = Account::from_bytes(
        key.try_into()
            .expect("invalid confirmation height key length"),
    );
    let mut bytes = value;
    let info = ConfirmationHeightInfo::deserialize(&mut bytes)
        .expect("failed to deserialize confirmation height");
    (account, info)
}

pub struct RocksdbRepWeightStore {
    database: StoreDatabase,
    delete_listener: OutputListenerMt<PublicKey>,
    put_listener: OutputListenerMt<(PublicKey, Amount)>,
}

impl RocksdbRepWeightStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let database = env.open_db(Some(REP_WEIGHT_CF_NAME))?;
        Ok(Self {
            database,
            delete_listener: OutputListenerMt::new(),
            put_listener: OutputListenerMt::new(),
        })
    }

    fn database(&self) -> StoreDatabase {
        self.database
    }

    pub fn track_deletions(&self) -> Arc<OutputTrackerMt<PublicKey>> {
        self.delete_listener.track()
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<(PublicKey, Amount)>> {
        self.put_listener.track()
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn, pub_key: &PublicKey) -> Option<Amount> {
        match txn.get(self.database(), pub_key.as_bytes()) {
            Ok(mut bytes) => Amount::deserialize(&mut bytes).ok(),
            Err(e) if e.is_not_found() => None,
            Err(e) => panic!("failed to read rep weight: {e}"),
        }
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, representative: PublicKey, weight: Amount) {
        if self.put_listener.is_tracked() {
            self.put_listener.emit((representative, weight));
        }
        txn.put(
            self.database(),
            representative.as_bytes(),
            &weight.to_be_bytes(),
            StoreWriteFlags::default(),
        )
        .expect("failed to write rep weight");
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, representative: &PublicKey) {
        if self.delete_listener.is_tracked() {
            self.delete_listener.emit(*representative);
        }
        txn.delete(self.database(), representative.as_bytes(), None)
            .expect("failed to delete rep weight");
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.count(self.database())
    }

    pub fn iter<'txn>(&'txn self, txn: &'txn dyn LedgerReadTxn) -> StoreIterator<'txn, (PublicKey, Amount)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open rep weight cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbRepWeightIterator::new(cursor))
    }
}

impl RepWeightStore for RocksdbRepWeightStore {
    fn get(&self, txn: &dyn LedgerReadTxn, rep: &PublicKey) -> Option<Amount> {
        RocksdbRepWeightStore::get(self, txn, rep)
    }

    fn put(&self, txn: &mut dyn LedgerWriteTxn, representative: PublicKey, weight: Amount) {
        RocksdbRepWeightStore::put(self, txn, representative, weight);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, representative: &PublicKey) {
        RocksdbRepWeightStore::del(self, txn, representative);
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(PublicKey, Amount)>> {
        RocksdbRepWeightStore::track_puts(self)
    }

    fn track_deletions(&self) -> Arc<OutputTrackerMt<PublicKey>> {
        RocksdbRepWeightStore::track_deletions(self)
    }
}

struct RocksdbRepWeightIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
}

impl<'txn> RocksdbRepWeightIterator<'txn> {
    fn new(cursor: RocksdbCursor<'txn>) -> Self {
        Self { cursor }
    }
}

impl<'txn> Iterator for RocksdbRepWeightIterator<'txn> {
    type Item = (PublicKey, Amount);

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.cursor.next().expect("failed to advance cursor")?;
        Some(read_rep_weight_record(entry))
    }
}

fn read_rep_weight_record((key, value): (&[u8], &[u8])) -> (PublicKey, Amount) {
    let pub_key = PublicKey::from_slice(
        key.try_into()
            .expect("invalid representative key length in RocksDB"),
    )
    .expect("failed to parse public key");
    let mut bytes = value;
    let amount = Amount::deserialize(&mut bytes).expect("failed to deserialize amount");
    (pub_key, amount)
}

pub struct RocksdbSuccessorStore {
    database: StoreDatabase,
    put_listener: OutputListenerMt<(BlockHash, BlockHash)>,
}

impl RocksdbSuccessorStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let database = env.open_db(Some(SUCCESSOR_CF_NAME))?;
        Ok(Self {
            database,
            put_listener: OutputListenerMt::new(),
        })
    }

    fn database(&self) -> StoreDatabase {
        self.database
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<(BlockHash, BlockHash)>> {
        self.put_listener.track()
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, block: &BlockHash, successor: &BlockHash) {
        if self.put_listener.is_tracked() {
            self.put_listener.emit((*block, *successor));
        }
        txn.put(
            self.database(),
            block.as_bytes(),
            successor.as_bytes(),
            StoreWriteFlags::default(),
        )
        .expect("failed to write successor");
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, block: &BlockHash) {
        txn.delete(self.database(), block.as_bytes(), None)
            .expect("failed to delete successor");
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn, block: &BlockHash) -> Option<BlockHash> {
        match txn.get(self.database(), block.as_bytes()) {
            Ok(bytes) => BlockHash::from_slice(bytes),
            Err(e) if e.is_not_found() => None,
            Err(e) => panic!("failed to read successor: {e}"),
        }
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.count(self.database())
    }
}

impl SuccessorStore for RocksdbSuccessorStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, block: &BlockHash, successor: &BlockHash) {
        RocksdbSuccessorStore::put(self, txn, block, successor);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, block: &BlockHash) {
        RocksdbSuccessorStore::del(self, txn, block);
    }

    fn get(&self, txn: &dyn LedgerReadTxn, block: &BlockHash) -> Option<BlockHash> {
        RocksdbSuccessorStore::get(self, txn, block)
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(BlockHash, BlockHash)>> {
        RocksdbSuccessorStore::track_puts(self)
    }
}

#[derive(Default)]
struct NullFinalVoteStore;

impl FinalVoteStore for NullFinalVoteStore {
    fn put(&self, _txn: &mut dyn LedgerWriteTxn, _root: &QualifiedRoot, _hash: &BlockHash) -> bool {
        false
    }

    fn get(&self, _txn: &dyn LedgerReadTxn, _root: &QualifiedRoot) -> Option<BlockHash> {
        None
    }
}

#[derive(Default)]
struct NullVersionStore;

impl VersionStore for NullVersionStore {
    fn get(&self, _txn: &dyn LedgerReadTxn) -> Option<i32> {
        None
    }
}

#[derive(Default)]
struct NullOnlineWeightStore;

impl OnlineWeightStore for NullOnlineWeightStore {
    fn put(&self, _txn: &mut dyn LedgerWriteTxn, _time: u64, _amount: &Amount) {}

    fn del(&self, _txn: &mut dyn LedgerWriteTxn, _time: u64) {}

    fn iter<'a>(&'a self, _txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, (u64, Amount)> {
        Box::new(iter::empty())
    }

    fn iter_rev<'a>(&'a self, _txn: &'a dyn LedgerReadTxn) -> StoreIterator<'a, (u64, Amount)> {
        Box::new(iter::empty())
    }
}

struct NullPeerStore {
    put_listener: OutputListenerMt<(SocketAddrV6, SystemTime)>,
    delete_listener: OutputListenerMt<SocketAddrV6>,
}

impl Default for NullPeerStore {
    fn default() -> Self {
        Self {
            put_listener: OutputListenerMt::new(),
            delete_listener: OutputListenerMt::new(),
        }
    }
}

impl PeerStore for NullPeerStore {
    fn put(&self, _txn: &mut dyn LedgerWriteTxn, _endpoint: SocketAddrV6, _time: SystemTime) {}

    fn del(&self, _txn: &mut dyn LedgerWriteTxn, _endpoint: SocketAddrV6) {}

    fn exists(&self, _txn: &dyn LedgerReadTxn, _endpoint: SocketAddrV6) -> bool {
        false
    }

    fn iter<'a>(
        &'a self,
        _txn: &'a dyn LedgerReadTxn,
    ) -> StoreIterator<'a, (SocketAddrV6, SystemTime)> {
        Box::new(iter::empty())
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(SocketAddrV6, SystemTime)>> {
        self.put_listener.track()
    }

    fn track_deletions(&self) -> Arc<OutputTrackerMt<SocketAddrV6>> {
        self.delete_listener.track()
    }
}

struct RocksdbLedgerStore {
    env: Arc<RocksdbStoreEnvironment>,
    cache: Arc<LedgerCache>,
    block: RocksdbBlockStore,
    account: RocksdbAccountStore,
    pending: RocksdbPendingStore,
    confirmation_height: RocksdbConfirmationHeightStore,
    rep_weight: Arc<RocksdbRepWeightStore>,
    successors: RocksdbSuccessorStore,
    final_vote: NullFinalVoteStore,
    peer: NullPeerStore,
    version: NullVersionStore,
    online_weight: NullOnlineWeightStore,
}

impl RocksdbLedgerStore {
    fn create(
        env: Arc<RocksdbStoreEnvironment>,
        cache: Arc<LedgerCache>,
    ) -> anyhow::Result<Arc<dyn LedgerStore>> {
        let block = RocksdbBlockStore::new(Arc::clone(&env))?;
        let account = RocksdbAccountStore::new(Arc::clone(&env))?;
        let pending = RocksdbPendingStore::new(Arc::clone(&env))?;
        let confirmation_height = RocksdbConfirmationHeightStore::new(Arc::clone(&env))?;
        let rep_weight = Arc::new(RocksdbRepWeightStore::new(Arc::clone(&env))?);
        let successors = RocksdbSuccessorStore::new(Arc::clone(&env))?;

        Ok(Arc::new(Self {
            env,
            cache,
            block,
            account,
            pending,
            confirmation_height,
            rep_weight,
            successors,
            final_vote: NullFinalVoteStore::default(),
            peer: NullPeerStore::default(),
            version: NullVersionStore::default(),
            online_weight: NullOnlineWeightStore::default(),
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

    fn begin_write(&self) -> Box<dyn LedgerWriteTxn> {
        Box::new(RocksdbLedgerWriteTxn::new(&self.env))
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
}

fn find_next_block_id(env: &Arc<RocksdbStoreEnvironment>, data_cf: StoreDatabase) -> Result<u64> {
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
                    key.try_into()
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

fn value_in_range<T>(value: &T, range: &RangeBounds<T>) -> bool
where
    T: Ord,
{
    use std::ops::Bound;
    let start_ok = match &range.start {
        Bound::Included(start) => value >= start,
        Bound::Excluded(start) => value > start,
        Bound::Unbounded => true,
    };
    let end_ok = match &range.end {
        Bound::Included(end) => value <= end,
        Bound::Excluded(end) => value < end,
        Bound::Unbounded => true,
    };
    start_ok && end_ok
}

pub struct RocksdbLedgerReadTxn {
    inner: RocksdbReadTxn<'static>,
}

impl RocksdbLedgerReadTxn {
    pub fn new(env: &Arc<RocksdbStoreEnvironment>) -> Self {
        let inner = env.inner();
        let txn = RocksdbReadTxn::new(&inner);
        let txn_static: RocksdbReadTxn<'static> = unsafe { mem::transmute(txn) };
        Self { inner: txn_static }
    }
}

pub struct RocksdbLedgerWriteTxn {
    inner: RocksdbWriteTxn<'static>,
}

impl RocksdbLedgerWriteTxn {
    pub fn new(env: &Arc<RocksdbStoreEnvironment>) -> Self {
        let inner = env.inner();
        let txn = RocksdbWriteTxn::new(&inner);
        let txn_static: RocksdbWriteTxn<'static> = unsafe { mem::transmute(txn) };
        Self { inner: txn_static }
    }

    pub fn as_inner_mut(&mut self) -> &mut RocksdbWriteTxn<'static> {
        &mut self.inner
    }
}

pub struct RocksdbStoreEnvironmentFactory;

impl Default for RocksdbStoreEnvironmentFactory {
    fn default() -> Self {
        Self
    }
}

impl StoreEnvironmentFactory for RocksdbStoreEnvironmentFactory {
    type Environment = RocksdbStoreEnvironment;

    fn create(&self, options: StoreEnvironmentOptions) -> anyhow::Result<Arc<Self::Environment>> {
        let StoreEnvironmentOptions { path, flags, .. } = options;
        let env = RocksdbStoreEnvironment::open(path, flags, None, None)?;
        Ok(Arc::new(env))
    }

    fn create_null(&self) -> Arc<Self::Environment> {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir for rocksdb env");
        let options = StoreEnvironmentOptions {
            path: temp_dir.path().to_path_buf(),
            max_databases: 128,
            map_size: 0,
            flags: StoreEnvironmentFlags::empty(),
        };
        let StoreEnvironmentOptions { path, flags, .. } = options;
        let env = RocksdbStoreEnvironment::open(path, flags, Some(temp_dir), None)
            .expect("temp RocksDB environment");
        Arc::new(env)
    }
}

pub struct RocksdbLedgerStoreFactory;

impl Default for RocksdbLedgerStoreFactory {
    fn default() -> Self {
        Self
    }
}

impl RocksdbLedgerStoreFactory {
    pub fn new() -> Self {
        Self
    }
}

impl LedgerStoreFactory for RocksdbLedgerStoreFactory {
    fn create_store(
        &self,
        path: PathBuf,
        config: LedgerStoreConfig,
        cache: Arc<LedgerCache>,
    ) -> anyhow::Result<Arc<dyn LedgerStore>> {
        let rocks_config = match config.backend {
            LedgerBackend::RocksDb(cfg) => cfg,
            _ => bail!("RocksDB factory requires RocksDB backend config"),
        };
        let env = RocksdbStoreEnvironment::open(
            path,
            StoreEnvironmentFlags::empty(),
            None,
            Some(&rocks_config),
        )?;
        RocksdbLedgerStore::create(Arc::new(env), cache)
    }

    fn create_null_store(&self, cache: Arc<LedgerCache>) -> anyhow::Result<Arc<dyn LedgerStore>> {
        let env_factory = RocksdbStoreEnvironmentFactory::default();
        let env = env_factory.create_null();
        RocksdbLedgerStore::create(env, cache)
    }
}

struct RocksDbInner {
    db: RocksDb,
    registry: RwLock<CfRegistry>,
}

type RocksDb = DBWithThreadMode<MultiThreaded>;
type RocksDbSnapshot<'a> = SnapshotWithThreadMode<'a, RocksDb>;

const BLOCK_INDEX_CF_NAME: &str = "rocksdb_block_index";
const BLOCK_DATA_CF_NAME: &str = "rocksdb_block_data";
const ACCOUNTS_CF_NAME: &str = "rocksdb_accounts";
const PENDING_CF_NAME: &str = "rocksdb_pending";
const CONF_HEIGHT_CF_NAME: &str = "rocksdb_confirmation_height";
const REP_WEIGHT_CF_NAME: &str = "rocksdb_rep_weights";
const SUCCESSOR_CF_NAME: &str = "rocksdb_successors";

enum WriteOp {
    Put {
        database: StoreDatabase,
        key: Vec<u8>,
        value: Vec<u8>,
    },
    Delete {
        database: StoreDatabase,
        key: Vec<u8>,
    },
    Clear {
        database: StoreDatabase,
    },
}

impl RocksDbInner {
    fn open(path: &Path, max_open_files: Option<i32>) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);
        if let Some(max_open_files) = max_open_files {
            options.set_max_open_files(max_open_files);
        }

        let cf_names = if path.exists() {
            RocksDb::list_cf(&options, path).unwrap_or_default()
        } else {
            Vec::new()
        };

        let db = if cf_names.is_empty() {
            let descriptor = ColumnFamilyDescriptor::new(
                rocksdb::DEFAULT_COLUMN_FAMILY_NAME,
                Options::default(),
            );
            RocksDb::open_cf_descriptors(&options, path, vec![descriptor])?
        } else {
            let descriptors = cf_names
                .iter()
                .map(|name| ColumnFamilyDescriptor::new(name, Options::default()))
                .collect::<Vec<_>>();
            RocksDb::open_cf_descriptors(&options, path, descriptors)?
        };

        let mut registry = CfRegistry::new();
        registry.insert_existing(rocksdb::DEFAULT_COLUMN_FAMILY_NAME);
        for name in cf_names {
            if name == rocksdb::DEFAULT_COLUMN_FAMILY_NAME {
                continue;
            }
            if db.cf_handle(&name).is_some() {
                registry.insert_existing(&name);
            }
        }

        Ok(Self {
            db,
            registry: RwLock::new(registry),
        })
    }

    fn snapshot(&self) -> RocksDbSnapshot<'_> {
        self.db.snapshot()
    }

    fn flush_wal(&self) -> StoreResult<()> {
        self.db.flush_wal(true).map_err(store_error_from_rocksdb)
    }

    fn open_database(&self, name: Option<&str>) -> StoreResult<StoreDatabase> {
        let name = name.unwrap_or(rocksdb::DEFAULT_COLUMN_FAMILY_NAME);
        if let Some(handle) = self.registry.read().handle_for_name(name) {
            return Ok(handle);
        }

        let mut cf_options = Options::default();
        cf_options.create_if_missing(true);
        self.db
            .create_cf(name, &cf_options)
            .map_err(store_error_from_rocksdb)?;
        self.db
            .cf_handle(name)
            .ok_or_else(|| StoreError::backend(format!("missing column family {name}")))?;
        Ok(self.registry.write().insert_existing(name))
    }

    fn cf_handle(&self, database: StoreDatabase) -> StoreResult<Arc<BoundColumnFamily<'_>>> {
        let name = {
            let registry = self.registry.read();
            registry
                .name_for_handle(database)
                .ok_or_else(|| StoreError::backend("invalid database handle"))?
        };
        self.db
            .cf_handle(&name)
            .ok_or_else(|| StoreError::backend(format!("missing column family {name}")))
    }

    fn collect_snapshot_entries(
        &self,
        snapshot: &RocksDbSnapshot<'_>,
        database: StoreDatabase,
    ) -> StoreResult<Vec<(Box<[u8]>, Box<[u8]>)>> {
        let handle = self.cf_handle(database)?;
        let iter = snapshot.iterator_cf(&handle, IteratorMode::Start);
        collect_entries(iter)
    }

    fn delete_cf(&self, database: StoreDatabase) -> StoreResult<()> {
        if database.into_raw().get() == 1 {
            return Err(StoreError::backend(
                "cannot drop default RocksDB column family",
            ));
        }

        let name = {
            let mut registry = self.registry.write();
            registry
                .remove(database)
                .ok_or_else(|| StoreError::backend("unknown column family"))?
        };
        self.db.drop_cf(&name).map_err(store_error_from_rocksdb)
    }
}

struct CfRegistry {
    next_id: usize,
    names_by_id: HashMap<usize, String>,
    ids_by_name: HashMap<String, usize>,
}

impl CfRegistry {
    fn new() -> Self {
        Self {
            next_id: 1,
            names_by_id: HashMap::new(),
            ids_by_name: HashMap::new(),
        }
    }

    fn insert_existing(&mut self, name: &str) -> StoreDatabase {
        if let Some(id) = self.ids_by_name.get(name) {
            return StoreDatabase::from_usize(*id).expect("non-zero id");
        }
        let id = self.next_id;
        self.next_id += 1;
        self.names_by_id.insert(id, name.to_string());
        self.ids_by_name.insert(name.to_string(), id);
        StoreDatabase::from_usize(id).expect("non-zero handle id")
    }

    fn handle_for_name(&self, name: &str) -> Option<StoreDatabase> {
        self.ids_by_name
            .get(name)
            .copied()
            .and_then(StoreDatabase::from_usize)
    }

    fn name_for_handle(&self, handle: StoreDatabase) -> Option<String> {
        let id = handle.into_raw().get();
        self.names_by_id.get(&id).cloned()
    }

    fn remove(&mut self, handle: StoreDatabase) -> Option<String> {
        let id = handle.into_raw().get();
        let name = self.names_by_id.remove(&id)?;
        self.ids_by_name.remove(&name);
        Some(name)
    }
}

pub struct RocksdbReadTxn<'env> {
    inner: Arc<RocksDbInner>,
    snapshot: RocksDbSnapshot<'env>,
    buffers: RefCell<Vec<Vec<u8>>>,
}

impl<'env> RocksdbReadTxn<'env> {
    fn new(inner: &'env Arc<RocksDbInner>) -> Self {
        let snapshot = inner.snapshot();
        Self {
            inner: Arc::clone(inner),
            snapshot,
            buffers: RefCell::new(Vec::new()),
        }
    }

    fn cache_bytes<'txn>(&'txn self, data: &[u8]) -> &'txn [u8]
    where
        'env: 'txn,
    {
        let mut buffers = self.buffers.borrow_mut();
        buffers.push(data.to_vec());
        let idx = buffers.len() - 1;
        let ptr: *const Vec<u8> = &buffers[idx];
        drop(buffers);
        unsafe { (&*ptr).as_slice() }
    }
}

impl<'env> StoreReadTxn<'env> for RocksdbReadTxn<'env> {
    type Cursor<'txn>
        = RocksdbCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

    fn get<'txn>(&'txn self, database: StoreDatabase, key: &[u8]) -> StoreResult<&'txn [u8]>
    where
        'env: 'txn,
    {
        let handle = self.inner.cf_handle(database)?;
        match self
            .snapshot
            .get_pinned_cf(&handle, key)
            .map_err(store_error_from_rocksdb)?
        {
            Some(value) => Ok(self.cache_bytes(value.as_ref())),
            None => Err(StoreError::not_found()),
        }
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        let entries = self
            .inner
            .collect_snapshot_entries(&self.snapshot, database)
            .unwrap_or_else(|e| panic!("failed to count RocksDB records: {e}"));
        entries.len() as u64
    }

    fn open_cursor<'txn>(&'txn self, database: StoreDatabase) -> StoreResult<Self::Cursor<'txn>>
    where
        'env: 'txn,
    {
        let entries = self
            .inner
            .collect_snapshot_entries(&self.snapshot, database)?;
        Ok(RocksdbCursor::new(entries))
    }

    fn commit(self)
    where
        Self: Sized,
    {
    }
}

pub struct RocksdbWriteTxn<'env> {
    inner: Arc<RocksDbInner>,
    snapshot: RocksDbSnapshot<'env>,
    batch: WriteBatch,
    buffers: RefCell<Vec<Vec<u8>>>,
    ops: Vec<WriteOp>,
}

impl<'env> RocksdbWriteTxn<'env> {
    fn new(inner: &'env Arc<RocksDbInner>) -> Self {
        let snapshot = inner.snapshot();
        Self {
            inner: Arc::clone(inner),
            snapshot,
            batch: WriteBatch::default(),
            buffers: RefCell::new(Vec::new()),
            ops: Vec::new(),
        }
    }

    fn lookup_overlay<'txn>(
        &'txn self,
        database: StoreDatabase,
        key: &[u8],
    ) -> Option<Option<&'txn [u8]>>
    where
        'env: 'txn,
    {
        for op in self.ops.iter().rev() {
            match op {
                WriteOp::Put {
                    database: db,
                    key: op_key,
                    value,
                } if *db == database && op_key.as_slice() == key => {
                    return Some(Some(self.cache_bytes(value.as_slice())));
                }
                WriteOp::Delete {
                    database: db,
                    key: op_key,
                } if *db == database && op_key.as_slice() == key => {
                    return Some(None);
                }
                WriteOp::Clear { database: db } if *db == database => {
                    return Some(None);
                }
                _ => {}
            }
        }
        None
    }

    fn cache_bytes<'txn>(&'txn self, data: &[u8]) -> &'txn [u8]
    where
        'env: 'txn,
    {
        let mut buffers = self.buffers.borrow_mut();
        buffers.push(data.to_vec());
        let idx = buffers.len() - 1;
        let ptr: *const Vec<u8> = &buffers[idx];
        drop(buffers);
        unsafe { (&*ptr).as_slice() }
    }

    fn apply_ops_to_map(
        &self,
        database: StoreDatabase,
        mut map: BTreeMap<Vec<u8>, Vec<u8>>,
    ) -> BTreeMap<Vec<u8>, Vec<u8>> {
        for op in &self.ops {
            match op {
                WriteOp::Put {
                    database: db,
                    key,
                    value,
                } if *db == database => {
                    map.insert(key.clone(), value.clone());
                }
                WriteOp::Delete { database: db, key } if *db == database => {
                    map.remove(key.as_slice());
                }
                WriteOp::Clear { database: db } if *db == database => {
                    map.clear();
                }
                _ => {}
            }
        }
        map
    }
}

impl<'env> StoreReadTxn<'env> for RocksdbWriteTxn<'env> {
    type Cursor<'txn>
        = RocksdbCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

    fn get<'txn>(&'txn self, database: StoreDatabase, key: &[u8]) -> StoreResult<&'txn [u8]>
    where
        'env: 'txn,
    {
        if let Some(result) = self.lookup_overlay(database, key) {
            return result.map_or(Err(StoreError::not_found()), Ok);
        }

        let handle = self.inner.cf_handle(database)?;
        match self
            .snapshot
            .get_pinned_cf(&handle, key)
            .map_err(store_error_from_rocksdb)?
        {
            Some(value) => Ok(self.cache_bytes(value.as_ref())),
            None => Err(StoreError::not_found()),
        }
    }

    fn count(&self, database: StoreDatabase) -> u64 {
        let entries = self
            .inner
            .collect_snapshot_entries(&self.snapshot, database)
            .unwrap_or_else(|e| panic!("failed to count RocksDB records: {e}"));
        let map: BTreeMap<Vec<u8>, Vec<u8>> = entries
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        self.apply_ops_to_map(database, map).len() as u64
    }

    fn open_cursor<'txn>(&'txn self, database: StoreDatabase) -> StoreResult<Self::Cursor<'txn>>
    where
        'env: 'txn,
    {
        let base_entries = self
            .inner
            .collect_snapshot_entries(&self.snapshot, database)?;
        let mut map: BTreeMap<Vec<u8>, Vec<u8>> = base_entries
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        map = self.apply_ops_to_map(database, map);
        let entries = map
            .into_iter()
            .map(|(k, v)| (k.into_boxed_slice(), v.into_boxed_slice()))
            .collect();
        Ok(RocksdbCursor::new(entries))
    }

    fn commit(self)
    where
        Self: Sized,
    {
        if let Err(err) = self.inner.db.write(self.batch) {
            panic!("failed to commit RocksDB batch: {err}");
        }
    }
}

impl<'env> StoreWriteTxn<'env> for RocksdbWriteTxn<'env> {
    type MutCursor<'txn>
        = RocksdbCursor<'txn>
    where
        Self: 'txn,
        'env: 'txn;

    fn put(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        value: &[u8],
        _flags: StoreWriteFlags,
    ) -> StoreResult<()> {
        let handle = self.inner.cf_handle(database)?;
        self.batch.put_cf(&handle, key, value);
        self.ops.push(WriteOp::Put {
            database,
            key: key.to_vec(),
            value: value.to_vec(),
        });
        Ok(())
    }

    fn delete(
        &mut self,
        database: StoreDatabase,
        key: &[u8],
        _value: Option<&[u8]>,
    ) -> StoreResult<()> {
        let handle = self.inner.cf_handle(database)?;
        self.batch.delete_cf(&handle, key);
        self.ops.push(WriteOp::Delete {
            database,
            key: key.to_vec(),
        });
        Ok(())
    }

    fn clear_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        let handle = self.inner.cf_handle(database)?;
        let mut iter = self.inner.db.iterator_cf(&handle, IteratorMode::Start);
        while let Some(item) = iter.next() {
            let (key, _) = item.map_err(store_error_from_rocksdb)?;
            self.batch.delete_cf(&handle, &key);
        }
        self.ops.push(WriteOp::Clear { database });
        Ok(())
    }

    fn open_rw_cursor<'txn>(
        &'txn mut self,
        database: StoreDatabase,
    ) -> StoreResult<Self::MutCursor<'txn>>
    where
        'env: 'txn,
    {
        self.open_cursor(database)
    }

    unsafe fn drop_db(&mut self, database: StoreDatabase) -> StoreResult<()> {
        self.inner.delete_cf(database)
    }
}

pub struct RocksdbCursor<'data> {
    entries: Vec<(Box<[u8]>, Box<[u8]>)>,
    index: usize,
    _marker: PhantomData<&'data ()>,
}

impl<'data> RocksdbCursor<'data> {
    fn new(entries: Vec<(Box<[u8]>, Box<[u8]>)>) -> Self {
        Self {
            entries,
            index: 0,
            _marker: PhantomData,
        }
    }
}

impl<'txn> StoreCursor<'txn> for RocksdbCursor<'txn> {
    fn next(&mut self) -> StoreResult<Option<(&'txn [u8], &'txn [u8])>> {
        if self.index >= self.entries.len() {
            return Ok(None);
        }
        let (key, value) = &self.entries[self.index];
        self.index += 1;
        let key_ref: &'txn [u8] = unsafe { mem::transmute::<&[u8], &'txn [u8]>(key.as_ref()) };
        let value_ref: &'txn [u8] = unsafe { mem::transmute::<&[u8], &'txn [u8]>(value.as_ref()) };
        Ok(Some((key_ref, value_ref)))
    }
}

fn store_ro_cursor_from_rocksdb<'txn>(cursor: RocksdbCursor<'txn>) -> StoreRoCursor<'txn> {
    let raw = Box::into_raw(Box::new(cursor)) as usize;
    let handle = unsafe { NonZeroUsize::new_unchecked(raw) };
    unsafe { StoreRoCursor::from_raw_parts(handle, drop_rocksdb_ro_cursor) }
}

fn store_rw_cursor_from_rocksdb<'txn>(cursor: RocksdbCursor<'txn>) -> StoreRwCursor<'txn> {
    let raw = Box::into_raw(Box::new(cursor)) as usize;
    let handle = unsafe { NonZeroUsize::new_unchecked(raw) };
    unsafe { StoreRwCursor::from_raw_parts(handle, drop_rocksdb_rw_cursor) }
}

fn rocksdb_ro_cursor_from_store<'txn>(cursor: StoreRoCursor<'txn>) -> RocksdbCursor<'txn> {
    let (handle, _) = cursor.into_raw_parts();
    let ptr = handle.get() as *mut RocksdbCursor<'txn>;
    *unsafe { Box::from_raw(ptr) }
}

unsafe fn drop_rocksdb_ro_cursor(handle: NonZeroUsize) {
    let ptr = handle.get() as *mut RocksdbCursor<'static>;
    unsafe {
        drop(Box::from_raw(ptr));
    }
}

unsafe fn drop_rocksdb_rw_cursor(handle: NonZeroUsize) {
    let ptr = handle.get() as *mut RocksdbCursor<'static>;
    unsafe {
        drop(Box::from_raw(ptr));
    }
}

fn collect_entries(
    iter: DBIteratorWithThreadMode<'_, RocksDb>,
) -> StoreResult<Vec<(Box<[u8]>, Box<[u8]>)>> {
    let mut entries = Vec::new();
    for item in iter {
        let (key, value) = item.map_err(store_error_from_rocksdb)?;
        entries.push((key, value));
    }
    Ok(entries)
}

fn store_error_from_rocksdb(err: RocksError) -> StoreError {
    let kind = match err.kind() {
        rocksdb::ErrorKind::NotFound => StoreErrorKind::NotFound,
        rocksdb::ErrorKind::InvalidArgument => StoreErrorKind::InvalidArgument,
        rocksdb::ErrorKind::Corruption => StoreErrorKind::Corruption,
        _ => StoreErrorKind::Backend,
    };
    StoreError::new(kind, err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_types::{Amount, Block, BlockHash, PrivateKey, PublicKey};
    use std::ops::Bound;
    use tempfile::tempdir;

    struct BlockFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbBlockStore,
    }

    impl BlockFixture {
        fn new() -> Self {
            let dir = tempdir().unwrap();
            let env = Arc::new(
                RocksdbStoreEnvironment::open(
                    dir.path().to_path_buf(),
                    StoreEnvironmentFlags::empty(),
                    Some(dir),
                    Some(&RocksDbConfig::default()),
                )
                .unwrap(),
            );
            let store = RocksdbBlockStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }
    }

    fn create_env() -> Arc<RocksdbStoreEnvironment> {
        let dir = tempdir().unwrap();
        let env = RocksdbStoreEnvironment::open(
            dir.path().to_path_buf(),
            StoreEnvironmentFlags::empty(),
            Some(dir),
            Some(&RocksDbConfig::default()),
        )
        .unwrap();
        Arc::new(env)
    }

    struct AccountFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbAccountStore,
    }

    impl AccountFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbAccountStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_accounts(&self, entries: &[(Account, AccountInfo)]) {
            let mut txn = self.begin_write();
            for (account, info) in entries {
                self.store.put(&mut txn, account, info);
            }
            Box::new(txn).commit();
        }
    }

    struct PendingFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbPendingStore,
    }

    impl PendingFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbPendingStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_entries(&self, entries: &[(PendingKey, PendingInfo)]) {
            let mut txn = self.begin_write();
            for (key, info) in entries {
                self.store.put(&mut txn, key, info);
            }
            Box::new(txn).commit();
        }
    }

    struct ConfirmationFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbConfirmationHeightStore,
    }

    impl ConfirmationFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbConfirmationHeightStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_entries(&self, entries: &[(Account, ConfirmationHeightInfo)]) {
            let mut txn = self.begin_write();
            for (account, info) in entries {
                self.store.put(&mut txn, account, info);
            }
            Box::new(txn).commit();
        }
    }

    struct RepWeightFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbRepWeightStore,
    }

    impl RepWeightFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbRepWeightStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_entries(&self, entries: &[(PublicKey, Amount)]) {
            let mut txn = self.begin_write();
            for (account, weight) in entries {
                self.store.put(&mut txn, *account, *weight);
            }
            Box::new(txn).commit();
        }
    }

    struct SuccessorFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbSuccessorStore,
    }

    impl SuccessorFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbSuccessorStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_entries(&self, entries: &[(BlockHash, BlockHash)]) {
            let mut txn = self.begin_write();
            for (block, successor) in entries {
                self.store.put(&mut txn, block, successor);
            }
            Box::new(txn).commit();
        }
    }

    #[test]
    fn write_and_read_roundtrip() {
        let env = create_env();
        let database = env.open_db(Some("blocks")).unwrap();

        {
            let mut txn = env.begin_write();
            txn.put(database, b"key", b"value", StoreWriteFlags::empty())
                .unwrap();
            txn.commit();
        }

        let txn = env.begin_read();
        assert_eq!(txn.get(database, b"key").unwrap(), b"value");
    }

    #[test]
    fn write_txn_drop_discards_changes() {
        let env = create_env();
        let database = env.open_db(Some("accounts")).unwrap();

        {
            let mut txn = env.begin_write();
            txn.put(database, b"key", b"value", StoreWriteFlags::empty())
                .unwrap();
            // Transaction dropped without commit
        }

        let read_txn = env.begin_read();
        let err = read_txn.get(database, b"key").unwrap_err();
        assert!(err.is_not_found());
    }

    #[test]
    fn write_txn_explicit_rollback() {
        let env = create_env();
        let database = env.open_db(Some("accounts")).unwrap();
        let mut txn = env.begin_write();
        txn.put(database, b"rollback", b"value", StoreWriteFlags::empty())
            .unwrap();
        drop(txn);

        let read_txn = env.begin_read();
        assert!(read_txn.get(database, b"rollback").is_err());
    }

    #[test]
    fn write_txn_reads_own_writes() {
        let env = create_env();
        let database = env.open_db(None).unwrap();

        let mut txn = env.begin_write();
        txn.put(database, b"pending", b"123", StoreWriteFlags::empty())
            .unwrap();
        assert_eq!(txn.get(database, b"pending").unwrap(), b"123");
    }

    #[test]
    fn read_txn_snapshot_isolation() {
        let env = create_env();
        let database = env.open_db(Some("pending")).unwrap();

        {
            let mut txn = env.begin_write();
            txn.put(database, b"snapshot", b"v1", StoreWriteFlags::empty())
                .unwrap();
            txn.commit();
        }

        let read_txn = env.begin_read();
        assert_eq!(read_txn.get(database, b"snapshot").unwrap(), b"v1");

        {
            let mut write_txn = env.begin_write();
            write_txn
                .put(database, b"snapshot", b"v2", StoreWriteFlags::empty())
                .unwrap();
            write_txn.commit();
        }

        // Existing read transaction should continue to see the original value.
        assert_eq!(read_txn.get(database, b"snapshot").unwrap(), b"v1");

        // A fresh read transaction gets the updated value.
        let fresh_read = env.begin_read();
        assert_eq!(fresh_read.get(database, b"snapshot").unwrap(), b"v2");
    }

    #[test]
    fn cursor_reflects_overlay() {
        let env = create_env();
        let database = env.open_db(Some("accounts")).unwrap();

        let mut txn = env.begin_write();
        txn.put(database, b"a", b"1", StoreWriteFlags::empty())
            .unwrap();
        txn.put(database, b"b", b"2", StoreWriteFlags::empty())
            .unwrap();
        let mut cursor = txn.open_rw_cursor(database).unwrap();

        let first = cursor.next().unwrap().unwrap();
        assert_eq!(first, (b"a".as_ref(), b"1".as_ref()));
        let second = cursor.next().unwrap().unwrap();
        assert_eq!(second, (b"b".as_ref(), b"2".as_ref()));
    }

    #[test]
    fn block_store_put_get() {
        let fixture = BlockFixture::new();
        let block = SavedBlock::new_test_open_block();
        let mut write_txn = fixture.begin_write();
        fixture.store.put(&mut write_txn, &block);
        Box::new(write_txn).commit();

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &block.hash()), Some(block));
    }

    #[test]
    fn block_store_delete() {
        let fixture = BlockFixture::new();
        let block = SavedBlock::new_test_open_block();
        let mut write_txn = fixture.begin_write();
        fixture.store.put(&mut write_txn, &block);
        Box::new(write_txn).commit();

        let mut delete_txn = fixture.begin_write();
        fixture.store.del(&mut delete_txn, &block.hash());
        Box::new(delete_txn).commit();

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &block.hash()).is_none());
    }

    #[test]
    fn block_store_iterates() {
        let fixture = BlockFixture::new();
        let mut write_txn = fixture.begin_write();
        for seed in 0..3 {
            let block = unique_block(seed);
            fixture.store.put(&mut write_txn, &block);
        }
        Box::new(write_txn).commit();

        let read_txn = fixture.begin_read();
        let count = fixture.store.iter(&read_txn).count();
        assert_eq!(count, 3);
    }

    #[test]
    fn account_store_put_get() {
        let fixture = AccountFixture::new();
        let tracker = fixture.store.track_puts();
        let account = Account::from(42);
        let info = AccountInfo::new_test_instance();

        let mut write_txn = fixture.begin_write();
        fixture.store.put(&mut write_txn, &account, &info);
        Box::new(write_txn).commit();

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &account), Some(info.clone()));
        assert_eq!(tracker.output(), vec![(account, info)]);
    }

    #[test]
    fn account_store_delete() {
        let fixture = AccountFixture::new();
        let entries = vec![
            (Account::from(1), AccountInfo::new_test_instance()),
            (Account::from(2), AccountInfo::new_test_instance()),
        ];
        fixture.insert_accounts(&entries);

        let mut write_txn = fixture.begin_write();
        fixture.store.del(&mut write_txn, &entries[0].0);
        Box::new(write_txn).commit();

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &entries[0].0).is_none());
        assert!(fixture.store.get(&read_txn, &entries[1].0).is_some());
    }

    #[test]
    fn account_store_iterates_in_order() {
        let fixture = AccountFixture::new();
        let entries = vec![
            (Account::from(1), AccountInfo::new_test_instance()),
            (Account::from(3), AccountInfo::new_test_instance()),
            (Account::from(2), AccountInfo::new_test_instance()),
        ];
        fixture.insert_accounts(&entries);

        let read_txn = fixture.begin_read();
        let accounts: Vec<_> = fixture
            .store
            .iter(&read_txn)
            .map(|(account, _)| account)
            .collect();
        assert_eq!(
            accounts,
            vec![Account::from(1), Account::from(2), Account::from(3)]
        );
    }

    #[test]
    fn account_store_iter_range() {
        let fixture = AccountFixture::new();
        let entries = vec![
            (Account::from(10), AccountInfo::new_test_instance()),
            (Account::from(20), AccountInfo::new_test_instance()),
            (Account::from(30), AccountInfo::new_test_instance()),
        ];
        fixture.insert_accounts(&entries);

        let read_txn = fixture.begin_read();
        let range = RangeBounds::new(
            Bound::Included(Account::from(15)),
            Bound::Excluded(Account::from(30)),
        );
        let accounts: Vec<_> = fixture
            .store
            .iter_range(&read_txn, range)
            .map(|(account, _)| account)
            .collect();
        assert_eq!(accounts, vec![Account::from(20)]);
    }

    #[test]
    fn account_store_count() {
        let fixture = AccountFixture::new();
        let entries = vec![
            (Account::from(1), AccountInfo::new_test_instance()),
            (Account::from(2), AccountInfo::new_test_instance()),
        ];
        fixture.insert_accounts(&entries);

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 2);
    }

    #[test]
    fn pending_store_not_found() {
        let fixture = PendingFixture::new();
        let read_txn = fixture.begin_read();
        let key = PendingKey::new_test_instance();
        assert!(fixture.store.get(&read_txn, &key).is_none());
        assert!(!fixture.store.exists(&read_txn, &key));
    }

    #[test]
    fn pending_store_put_get() {
        let fixture = PendingFixture::new();
        let key = PendingKey::new_test_instance();
        let info = PendingInfo::new_test_instance();
        let mut write_txn = fixture.begin_write();
        let tracker = fixture.store.track_puts();
        fixture.store.put(&mut write_txn, &key, &info);
        Box::new(write_txn).commit();

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &key), Some(info.clone()));
        assert_eq!(tracker.output(), vec![(key, info)]);
    }

    #[test]
    fn pending_store_delete() {
        let fixture = PendingFixture::new();
        let key = PendingKey::new_test_instance();
        let info = PendingInfo::new_test_instance();
        fixture.insert_entries(&[(key.clone(), info)]);

        let mut write_txn = fixture.begin_write();
        let tracker = fixture.store.track_deletions();
        fixture.store.del(&mut write_txn, &key);
        Box::new(write_txn).commit();
        assert_eq!(tracker.output(), vec![key.clone()]);

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &key).is_none());
    }

    #[test]
    fn pending_store_iter_empty() {
        let fixture = PendingFixture::new();
        let read_txn = fixture.begin_read();
        assert!(fixture.store.iter(&read_txn).next().is_none());
    }

    #[test]
    fn pending_store_iterates() {
        let fixture = PendingFixture::new();
        let key = PendingKey::new_test_instance();
        let info = PendingInfo::new_test_instance();
        fixture.insert_entries(&[(key.clone(), info.clone())]);

        let read_txn = fixture.begin_read();
        let entries: Vec<_> = fixture.store.iter(&read_txn).collect();
        assert_eq!(entries, vec![(key, info)]);
    }

    #[test]
    fn pending_store_iter_range() {
        let fixture = PendingFixture::new();
        let k1 = PendingKey::new(Account::from(1), BlockHash::from(1));
        let k2 = PendingKey::new(Account::from(2), BlockHash::from(1));
        let k3 = PendingKey::new(Account::from(3), BlockHash::from(1));
        let info = PendingInfo::new_test_instance();
        fixture.insert_entries(&[(k1, info.clone()), (k2, info.clone()), (k3, info.clone())]);

        let read_txn = fixture.begin_read();
        let range = RangeBounds::new(
            Bound::Included(PendingKey::new(Account::from(2), BlockHash::from(0))),
            Bound::Excluded(PendingKey::new(Account::from(3), BlockHash::from(0))),
        );
        let entries: Vec<_> = fixture.store.iter_range(&read_txn, range).collect();
        assert_eq!(entries, vec![(k2, info)]);
    }

    #[test]
    fn pending_store_exists() {
        let fixture = PendingFixture::new();
        let key = PendingKey::new_test_instance();
        let info = PendingInfo::new_test_instance();
        fixture.insert_entries(&[(key.clone(), info)]);
        let read_txn = fixture.begin_read();
        assert!(fixture.store.exists(&read_txn, &key));
    }

    #[test]
    fn pending_store_any_for_account() {
        let fixture = PendingFixture::new();
        let account = Account::from(42);
        let key = PendingKey::new(account, BlockHash::from(7));
        let info = PendingInfo::new_test_instance();
        fixture.insert_entries(&[(key, info)]);

        let read_txn = fixture.begin_read();
        assert!(fixture.store.any(&read_txn, &account));
        assert!(!fixture.store.any(&read_txn, &Account::from(5)));
    }

    #[test]
    fn confirmation_store_empty() {
        let fixture = ConfirmationFixture::new();
        let read_txn = fixture.begin_read();
        let account = Account::from(1);
        assert!(fixture.store.get(&read_txn, &account).is_none());
        assert!(!fixture.store.exists(&read_txn, &account));
        assert!(fixture.store.iter(&read_txn).next().is_none());
    }

    #[test]
    fn confirmation_store_put_get() {
        let fixture = ConfirmationFixture::new();
        let account = Account::from(2);
        let info = ConfirmationHeightInfo::new(5, BlockHash::from(9));
        let mut txn = fixture.begin_write();
        fixture.store.put(&mut txn, &account, &info);
        Box::new(txn).commit();

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &account), Some(info.clone()));
        assert!(fixture.store.exists(&read_txn, &account));
        assert_eq!(fixture.store.count(&read_txn), 1);
    }

    #[test]
    fn confirmation_store_delete() {
        let fixture = ConfirmationFixture::new();
        let account = Account::from(3);
        let info = ConfirmationHeightInfo::new(2, BlockHash::from(5));
        fixture.insert_entries(&[(account, info)]);

        let mut txn = fixture.begin_write();
        fixture.store.del(&mut txn, &Account::from(3));
        Box::new(txn).commit();

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &Account::from(3)).is_none());
    }

    #[test]
    fn confirmation_store_iter_range() {
        let fixture = ConfirmationFixture::new();
        let entries = vec![
            (Account::from(1), ConfirmationHeightInfo::new(1, BlockHash::from(1))),
            (Account::from(2), ConfirmationHeightInfo::new(2, BlockHash::from(2))),
            (Account::from(3), ConfirmationHeightInfo::new(3, BlockHash::from(3))),
        ];
        fixture.insert_entries(&entries);

        let read_txn = fixture.begin_read();
        let range = RangeBounds::new(
            Bound::Included(Account::from(2)),
            Bound::Excluded(Account::from(3)),
        );
        let entries: Vec<_> = fixture.store.iter_range(&read_txn, range).collect();
        assert_eq!(
            entries,
            vec![(Account::from(2), ConfirmationHeightInfo::new(2, BlockHash::from(2)))]
        );
    }

    #[test]
    fn confirmation_store_clear() {
        let fixture = ConfirmationFixture::new();
        let entries = vec![(
            Account::from(1),
            ConfirmationHeightInfo::new(1, BlockHash::from(1)),
        )];
        fixture.insert_entries(&entries);

        let mut txn = fixture.begin_write();
        fixture.store.clear(&mut txn);
        Box::new(txn).commit();

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 0);
    }

    #[test]
    fn rep_weight_count() {
        let fixture = RepWeightFixture::new();
        let entries = vec![
            (PublicKey::from(1), Amount::from(10)),
            (PublicKey::from(2), Amount::from(20)),
        ];
        fixture.insert_entries(&entries);
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 2);
    }

    #[test]
    fn rep_weight_put_get() {
        let fixture = RepWeightFixture::new();
        let mut write_txn = fixture.begin_write();
        let put_tracker = fixture.store.track_puts();
        let account = PublicKey::from(5);
        let weight = Amount::from(50);
        fixture.store.put(&mut write_txn, account, weight);
        Box::new(write_txn).commit();

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &account), Some(weight));
        assert_eq!(put_tracker.output(), vec![(account, weight)]);
    }

    #[test]
    fn rep_weight_delete() {
        let fixture = RepWeightFixture::new();
        let account = PublicKey::from(7);
        fixture.insert_entries(&[(account, Amount::from(70))]);

        let mut write_txn = fixture.begin_write();
        let delete_tracker = fixture.store.track_deletions();
        fixture.store.del(&mut write_txn, &account);
        Box::new(write_txn).commit();

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &account).is_none());
        assert_eq!(delete_tracker.output(), vec![account]);
    }

    #[test]
    fn rep_weight_iter_empty() {
        let fixture = RepWeightFixture::new();
        let read_txn = fixture.begin_read();
        assert!(fixture.store.iter(&read_txn).next().is_none());
    }

    #[test]
    fn rep_weight_iterates() {
        let fixture = RepWeightFixture::new();
        let entries = vec![
            (PublicKey::from(1), Amount::from(100)),
            (PublicKey::from(2), Amount::from(200)),
        ];
        fixture.insert_entries(&entries);

        let read_txn = fixture.begin_read();
        let items: Vec<_> = fixture.store.iter(&read_txn).collect();
        assert_eq!(items, entries);
    }

    #[test]
    fn successor_store_count() {
        let fixture = SuccessorFixture::new();
        let entries = vec![
            (BlockHash::from(1), BlockHash::from(2)),
            (BlockHash::from(3), BlockHash::from(4)),
        ];
        fixture.insert_entries(&entries);
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 2);
    }

    #[test]
    fn successor_store_put_get() {
        let fixture = SuccessorFixture::new();
        let mut txn = fixture.begin_write();
        let tracker = fixture.store.track_puts();
        let block = BlockHash::from(10);
        let successor = BlockHash::from(11);
        fixture.store.put(&mut txn, &block, &successor);
        Box::new(txn).commit();

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &block), Some(successor));
        assert_eq!(tracker.output(), vec![(block, successor)]);
    }

    #[test]
    fn successor_store_delete() {
        let fixture = SuccessorFixture::new();
        let block = BlockHash::from(5);
        let successor = BlockHash::from(6);
        fixture.insert_entries(&[(block, successor)]);

        let mut txn = fixture.begin_write();
        fixture.store.del(&mut txn, &block);
        Box::new(txn).commit();

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &block).is_none());
    }

    #[test]
    fn successor_store_no_entry() {
        let fixture = SuccessorFixture::new();
        let read_txn = fixture.begin_read();
        assert!(fixture
            .store
            .get(&read_txn, &BlockHash::from(999))
            .is_none());
    }

    fn unique_block(seed: u8) -> SavedBlock {
        let key = PrivateKey::from(u64::from(seed) + 42);
        let block = Block::new_test_instance_with_key(key);
        SavedBlock::new_test_instance_with(block)
    }
}
