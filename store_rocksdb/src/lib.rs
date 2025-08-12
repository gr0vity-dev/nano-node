use anyhow::Result;
use rocksdb::{Options, DB};
use std::sync::Arc;
use store_api::{ReadTxnLike, StoreProvider, TransactionLike, VersionStore, WriteTxnLike};

pub struct RocksProvider {
    db: Arc<DB>,
    version: RocksVersionStore,
}

impl RocksProvider {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        let db = Arc::new(DB::open(&opts, path)?);
        Ok(Self { version: RocksVersionStore { db: db.clone() }, db })
    }
}

use rocksdb::WriteBatch;

pub struct RocksReadTxn;
pub struct RocksWriteTxn {
    batch: WriteBatch,
}

impl TransactionLike for RocksReadTxn {
    fn is_refresh_needed(&self) -> bool { false }
}
impl ReadTxnLike for RocksReadTxn {}

impl TransactionLike for RocksWriteTxn {
    fn is_refresh_needed(&self) -> bool { false }
}
impl WriteTxnLike for RocksWriteTxn {}

impl StoreProvider for RocksProvider {
    type ReadTxn = RocksReadTxn;
    type WriteTxn = RocksWriteTxn;

    fn begin_read(&self) -> Self::ReadTxn { RocksReadTxn }
    fn begin_write(&self) -> Self::WriteTxn { RocksWriteTxn { batch: WriteBatch::default() } }
    fn refresh(&self, write: Self::WriteTxn) -> Self::WriteTxn { write }
    fn commit(&self, write: Self::WriteTxn) {
        let _ = self.db.write(write.batch);
        let _ = self.db.flush();
    }

    type Version = RocksVersionStore;
    fn version(&self) -> &Self::Version { &self.version }
}

pub struct RocksVersionStore {
    db: Arc<DB>,
}

const META_VERSION_KEY: &[u8] = b"meta:version";

impl VersionStore<RocksReadTxn, RocksWriteTxn> for RocksVersionStore {
    fn get(&self, _read: &RocksReadTxn) -> Option<i32> {
        self.db.get(META_VERSION_KEY).ok().flatten().map(|v| {
            let mut arr = [0u8; 4];
            arr.copy_from_slice(&v);
            i32::from_be_bytes(arr)
        })
    }

    fn set(&self, write: &mut RocksWriteTxn, version: i32) {
        let _ = write.batch.put(META_VERSION_KEY, version.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn version_roundtrip() -> Result<()> {
        let dir = tempdir()?;
        let provider = RocksProvider::open(dir.path())?;

        let mut w = provider.begin_write();
        provider.version().set(&mut w, 777);

        // Not visible until commit
        let r = provider.begin_read();
        assert_eq!(provider.version().get(&r), None);

        provider.commit(w);

        let r = provider.begin_read();
        assert_eq!(provider.version().get(&r), Some(777));
        Ok(())
    }
}
