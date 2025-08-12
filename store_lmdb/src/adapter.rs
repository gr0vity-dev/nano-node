#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_nullable_lmdb::{EnvironmentFlags, EnvironmentOptions, LmdbEnvironmentFactory};

    #[test]
    fn version_roundtrip_via_trait() -> anyhow::Result<()> {
        let options = EnvironmentOptions {
            max_dbs: 10,
            map_size: 1024 * 1024,
            flags: EnvironmentFlags::empty(),
            path: "/nulled/adapter.ldb".into(),
        };
        let env = LmdbEnvironmentFactory::new_null().create(options)?;
        let store = LmdbStore::new(env)?;

        let provider: &dyn StoreProvider<ReadTxn = ReadTxnPub, WriteTxn = WriteTxnPub> = &store;

        // set
        let mut w = provider.begin_write();
        provider.version().set(&mut w, 12345);
        provider.commit(w);

        // get
        let r = provider.begin_read();
        let got = provider.version().get(&r);
        assert_eq!(got, Some(12345));
        Ok(())
    }
}
use crate::{store::LmdbStore, version_store::LmdbVersionStore};
use rsnano_nullable_lmdb::{ReadTransaction, WriteTransaction};
use store_api::{ReadTxnLike, StoreProvider, TransactionLike, VersionStore, WriteTxnLike};

pub struct ReadTxnPub(pub ReadTransaction);
pub struct WriteTxnPub(pub WriteTransaction);

impl TransactionLike for ReadTxnPub {
    fn is_refresh_needed(&self) -> bool { rsnano_nullable_lmdb::Transaction::is_refresh_needed(&self.0) }
    fn as_any(&self) -> &dyn std::any::Any { &self.0 }
}

impl ReadTxnLike for ReadTxnPub {}

impl TransactionLike for WriteTxnPub {
    fn is_refresh_needed(&self) -> bool { rsnano_nullable_lmdb::Transaction::is_refresh_needed(&self.0) }
    fn as_any(&self) -> &dyn std::any::Any { &self.0 }
}

impl WriteTxnLike for WriteTxnPub {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { &mut self.0 }
}

impl VersionStore for LmdbVersionStore {
    fn get(&self, read: &dyn ReadTxnLike) -> Option<i32> {
        // Downcast: our ReadTxnLike is backed by ReadTransaction
        let read = read.as_any().downcast_ref::<ReadTransaction>().expect("Expected ReadTransaction");
        self.get(read)
    }

    fn set(&self, write: &mut dyn WriteTxnLike, version: i32) {
        let write = write.as_any_mut().downcast_mut::<WriteTransaction>().expect("Expected WriteTransaction");
        self.put(write, version)
    }
}

impl StoreProvider for LmdbStore {
    type ReadTxn = ReadTxnPub;
    type WriteTxn = WriteTxnPub;

    fn begin_read(&self) -> Self::ReadTxn { ReadTxnPub(self.begin_read()) }

    fn begin_write(&self) -> Self::WriteTxn { WriteTxnPub(self.begin_write()) }

    fn refresh(&self, write: Self::WriteTxn) -> Self::WriteTxn { WriteTxnPub(self.env.refresh(write.0)) }

    fn commit(&self, write: Self::WriteTxn) { write.0.commit(); }

    fn version(&self) -> &dyn VersionStore { &self.version }
}

