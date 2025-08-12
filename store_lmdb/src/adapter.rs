use crate::{store::LmdbStore, version_store::LmdbVersionStore};
use rsnano_nullable_lmdb::{ReadTransaction, WriteTransaction};
use store_api::{ReadTxnLike, RepWeightStore as RepWeightStoreApi, StoreProvider, TransactionLike, VersionStore, WriteTxnLike, PrunedStore as PrunedStoreApi};
use rsnano_core::{PublicKey, Amount};
use crate::{LmdbRepWeightStore, LmdbPrunedStore};

pub struct ReadTxnPub(pub ReadTransaction);
pub struct WriteTxnPub(pub WriteTransaction);

impl TransactionLike for ReadTxnPub {
    fn is_refresh_needed(&self) -> bool { rsnano_nullable_lmdb::Transaction::is_refresh_needed(&self.0) }
}

impl ReadTxnLike for ReadTxnPub {}

impl TransactionLike for WriteTxnPub {
    fn is_refresh_needed(&self) -> bool { rsnano_nullable_lmdb::Transaction::is_refresh_needed(&self.0) }
}

impl WriteTxnLike for WriteTxnPub {}

impl VersionStore<ReadTxnPub, WriteTxnPub> for LmdbVersionStore {
    fn get(&self, read: &ReadTxnPub) -> Option<i32> { self.get(&read.0) }
    fn set(&self, write: &mut WriteTxnPub, version: i32) { self.put(&mut write.0, version) }
}

impl StoreProvider for LmdbStore {
    type ReadTxn = ReadTxnPub;
    type WriteTxn = WriteTxnPub;

    fn begin_read(&self) -> Self::ReadTxn { ReadTxnPub(self.begin_read()) }

    fn begin_write(&self) -> Self::WriteTxn { WriteTxnPub(self.begin_write()) }

    fn refresh(&self, write: Self::WriteTxn) -> Self::WriteTxn { WriteTxnPub(self.env.refresh(write.0)) }

    fn commit(&self, write: Self::WriteTxn) { write.0.commit(); }

    type Version = LmdbVersionStore;
    fn version(&self) -> &Self::Version { &self.version }
    type Pruned = LmdbPrunedStore;
    fn pruned(&self) -> &Self::Pruned { &self.pruned }
}

impl RepWeightStoreApi<ReadTxnPub, WriteTxnPub> for LmdbRepWeightStore {
    fn get(&self, read: &ReadTxnPub, rep: &PublicKey) -> Option<Amount> { self.get(&read.0, rep) }
    fn put(&self, write: &mut WriteTxnPub, rep: PublicKey, weight: Amount) { self.put(&mut write.0, rep, weight) }
    fn del(&self, write: &mut WriteTxnPub, rep: &PublicKey) { self.del(&mut write.0, rep) }
    fn count(&self, read: &ReadTxnPub) -> u64 { self.count(&read.0) }
}

impl PrunedStoreApi<ReadTxnPub, WriteTxnPub> for LmdbPrunedStore {
    fn count(&self, read: &ReadTxnPub) -> u64 { self.count(&read.0) }
    fn exists(&self, read: &ReadTxnPub, hash: &rsnano_core::BlockHash) -> bool { self.exists(&read.0, hash) }
    fn put(&self, write: &mut WriteTxnPub, hash: &rsnano_core::BlockHash) { self.put(&mut write.0, hash) }
    fn del(&self, write: &mut WriteTxnPub, hash: &rsnano_core::BlockHash) { self.del(&mut write.0, hash) }
}

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

        let provider: &dyn StoreProvider<ReadTxn = ReadTxnPub, WriteTxn = WriteTxnPub, Version = LmdbVersionStore, Pruned = LmdbPrunedStore> = &store;

        // set via trait
        let mut w = provider.begin_write();
        <LmdbVersionStore as VersionStore<ReadTxnPub, WriteTxnPub>>::set(provider.version(), &mut w, 12345);
        provider.commit(w);

        // get via trait
        let r = provider.begin_read();
        let got = <LmdbVersionStore as VersionStore<ReadTxnPub, WriteTxnPub>>::get(provider.version(), &r);
        assert_eq!(got, Some(12345));
        Ok(())
    }
}

