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

pub struct RocksReadTxn;
pub struct RocksWriteTxn;

impl TransactionLike for RocksReadTxn {
    fn is_refresh_needed(&self) -> bool { false }
    fn as_any(&self) -> &dyn std::any::Any { self }
}
impl ReadTxnLike for RocksReadTxn {}

impl TransactionLike for RocksWriteTxn {
    fn is_refresh_needed(&self) -> bool { false }
    fn as_any(&self) -> &dyn std::any::Any { self }
}
impl WriteTxnLike for RocksWriteTxn {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

impl StoreProvider for RocksProvider {
    type ReadTxn = RocksReadTxn;
    type WriteTxn = RocksWriteTxn;

    fn begin_read(&self) -> Self::ReadTxn { RocksReadTxn }
    fn begin_write(&self) -> Self::WriteTxn { RocksWriteTxn }
    fn refresh(&self, write: Self::WriteTxn) -> Self::WriteTxn { write }
    fn commit(&self, _write: Self::WriteTxn) {}

    fn version(&self) -> &dyn VersionStore { &self.version }
}

pub struct RocksVersionStore {
    db: Arc<DB>,
}

const META_VERSION_KEY: &[u8] = b"meta:version";

impl VersionStore for RocksVersionStore {
    fn get(&self, _read: &dyn ReadTxnLike) -> Option<i32> {
        self.db.get(META_VERSION_KEY).ok().flatten().map(|v| {
            let mut arr = [0u8; 4];
            arr.copy_from_slice(&v);
            i32::from_be_bytes(arr)
        })
    }

    fn set(&self, _write: &mut dyn WriteTxnLike, version: i32) {
        let _ = self.db.put(META_VERSION_KEY, version.to_be_bytes());
        let _ = self.db.flush();
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
        provider.commit(w);

        let r = provider.begin_read();
        assert_eq!(provider.version().get(&r), Some(777));
        Ok(())
    }
}
