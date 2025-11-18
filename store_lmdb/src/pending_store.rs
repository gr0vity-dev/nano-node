use std::{ops::RangeBounds, sync::Arc};

use rsnano_nullable_lmdb::{
    ConfiguredDatabase, DatabaseFlags, LmdbDatabase, LmdbEnvironment, WriteFlags,
};
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::{Account, BlockHash, PendingInfo, PendingKey};
use store_traits::{
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::StoreDatabase,
};

use crate::{
    LmdbIterator, PENDING_TEST_DATABASE,
    iterator::LmdbRangeIterator,
    store_utils::{lmdb_ro_cursor_from_store, store_database_from_lmdb, store_write_flags_from},
};

pub struct LmdbPendingStore {
    database: LmdbDatabase,
    put_listener: OutputListenerMt<(PendingKey, PendingInfo)>,
    delete_listener: OutputListenerMt<PendingKey>,
}

impl LmdbPendingStore {
    pub fn new(env: &LmdbEnvironment) -> anyhow::Result<Self> {
        let database = env.create_db(Some("pending"), DatabaseFlags::empty())?;

        Ok(Self {
            database,
            put_listener: OutputListenerMt::new(),
            delete_listener: OutputListenerMt::new(),
        })
    }

    pub fn database(&self) -> LmdbDatabase {
        self.database
    }

    fn store_database(&self) -> StoreDatabase {
        store_database_from_lmdb(self.database)
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<(PendingKey, PendingInfo)>> {
        self.put_listener.track()
    }

    pub fn track_deletions(&self) -> Arc<OutputTrackerMt<PendingKey>> {
        self.delete_listener.track()
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey, pending: &PendingInfo) {
        self.put_listener.emit((key.clone(), pending.clone()));
        let key_bytes = key.to_bytes();
        let pending_bytes = pending.to_bytes();
        txn.put(
            self.store_database(),
            &key_bytes,
            &pending_bytes,
            store_write_flags_from(WriteFlags::empty()),
        )
        .unwrap();
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, key: &PendingKey) {
        self.delete_listener.emit(key.clone());
        let key_bytes = key.to_bytes();
        txn.delete(self.store_database(), &key_bytes, None).unwrap();
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn, key: &PendingKey) -> Option<PendingInfo> {
        let key_bytes = key.to_bytes();
        match txn.get(self.store_database(), &key_bytes) {
            Ok(bytes) => {
                let mut slice = bytes.as_ref();
                Some(
                    PendingInfo::deserialize(&mut slice).expect("Should be valid pending info"),
                )
            }
            Err(e) if e.is_not_found() => None,
            Err(e) => {
                panic!("Could not load pending info: {:?}", e);
            }
        }
    }

    pub fn iter<'tx>(
        &self,
        tx: &'tx dyn LedgerReadTxn,
    ) -> impl Iterator<Item = (PendingKey, PendingInfo)> + 'tx + use<'tx> {
        let cursor = tx.open_ro_cursor(self.store_database()).unwrap();
        let cursor = lmdb_ro_cursor_from_store(cursor);
        LmdbIterator::new(cursor, read_pending_record)
    }

    pub fn iter_range<'tx>(
        &self,
        tx: &'tx dyn LedgerReadTxn,
        range: impl RangeBounds<PendingKey> + 'static,
    ) -> impl Iterator<Item = (PendingKey, PendingInfo)> + 'tx {
        let cursor = tx.open_ro_cursor(self.store_database()).unwrap();
        let cursor = lmdb_ro_cursor_from_store(cursor);
        LmdbRangeIterator::new(
            cursor,
            range.start_bound().map(|b| b.to_bytes().to_vec()),
            range.end_bound().map(|b| b.to_bytes().to_vec()),
            read_pending_record,
        )
    }

    pub fn exists(&self, txn: &dyn LedgerReadTxn, key: &PendingKey) -> bool {
        self.iter_range(txn, *key..)
            .next()
            .map(|(k, _)| k == *key)
            .unwrap_or(false)
    }

    pub fn any(&self, tx: &dyn LedgerReadTxn, account: &Account) -> bool {
        let key = PendingKey::new(*account, BlockHash::ZERO);
        self.iter_range(tx, key..)
            .next()
            .map(|(k, _)| k.receiving_account == *account)
            .unwrap_or(false)
    }
}

pub struct ConfiguredPendingDatabaseBuilder {
    database: ConfiguredDatabase,
}

impl ConfiguredPendingDatabaseBuilder {
    pub fn new() -> Self {
        Self {
            database: ConfiguredDatabase::new(PENDING_TEST_DATABASE, "pending"),
        }
    }

    pub fn pending(mut self, key: &PendingKey, info: &PendingInfo) -> Self {
        self.database.insert(key.to_bytes(), info.to_bytes());
        self
    }

    pub fn build(self) -> ConfiguredDatabase {
        self.database
    }

    pub fn create(frontiers: Vec<(PendingKey, PendingInfo)>) -> ConfiguredDatabase {
        let mut builder = Self::new();
        for (key, info) in frontiers {
            builder = builder.pending(&key, &info);
        }
        builder.build()
    }
}

pub fn read_pending_record(mut key: &[u8], mut value: &[u8]) -> (PendingKey, PendingInfo) {
    let key = PendingKey::deserialize(&mut key).unwrap();
    let info = PendingInfo::deserialize(&mut value).unwrap();
    (key, info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::{LmdbLedgerReadTxn, LmdbLedgerWriteTxn};
    use rsnano_nullable_lmdb::{DeleteEvent, PutEvent};

    struct Fixture {
        env: Arc<LmdbEnvironment>,
        store: LmdbPendingStore,
    }

    impl Fixture {
        pub fn new() -> Self {
            Self::with_stored_data(Vec::new())
        }

        pub fn with_stored_data(entries: Vec<(PendingKey, PendingInfo)>) -> Self {
            let env = LmdbEnvironment::null_builder()
                .configured_database(ConfiguredPendingDatabaseBuilder::create(entries))
                .build();

            let env = Arc::new(env);
            Self {
                env: env.clone(),
                store: LmdbPendingStore::new(&env).unwrap(),
            }
        }

        fn begin_read(&self) -> LmdbLedgerReadTxn {
            LmdbLedgerReadTxn::new(self.env.begin_read())
        }

        fn begin_write(&self) -> LmdbLedgerWriteTxn {
            LmdbLedgerWriteTxn::new(self.env.begin_write())
        }
    }

    #[test]
    fn not_found() {
        let fixture = Fixture::new();
        let txn = fixture.begin_read();
        let result = fixture.store.get(&txn, &PendingKey::new_test_instance());
        assert!(result.is_none());
        assert_eq!(
            fixture.store.exists(&txn, &PendingKey::new_test_instance()),
            false
        );
    }

    #[test]
    fn load_pending_info() {
        let key = PendingKey::new_test_instance();
        let info = PendingInfo::new_test_instance();
        let fixture = Fixture::with_stored_data(vec![(key.clone(), info.clone())]);
        let txn = fixture.begin_read();

        let result = fixture.store.get(&txn, &key);

        assert_eq!(result, Some(info));
        assert_eq!(fixture.store.exists(&txn, &key), true);
    }

    #[test]
    fn add_pending() {
        let fixture = Fixture::new();
        let mut txn = fixture.begin_write();
        let put_tracker = txn.as_inner_mut().track_puts();
        let pending_key = PendingKey::new_test_instance();
        let pending = PendingInfo::new_test_instance();

        fixture.store.put(&mut txn, &pending_key, &pending);

        assert_eq!(
            put_tracker.output(),
            vec![PutEvent {
                database: PENDING_TEST_DATABASE.into(),
                key: pending_key.to_bytes().to_vec(),
                value: pending.to_bytes().to_vec(),
                flags: WriteFlags::empty()
            }]
        );
    }

    #[test]
    fn delete() {
        let fixture = Fixture::new();
        let mut txn = fixture.begin_write();
        let delete_tracker = txn.as_inner_mut().track_deletions();
        let pending_key = PendingKey::new_test_instance();

        fixture.store.del(&mut txn, &pending_key);

        assert_eq!(
            delete_tracker.output(),
            vec![DeleteEvent {
                database: PENDING_TEST_DATABASE.into(),
                key: pending_key.to_bytes().to_vec()
            }]
        )
    }

    #[test]
    fn iter_empty() {
        let fixture = Fixture::new();
        let tx = fixture.begin_read();
        assert!(fixture.store.iter(&tx).next().is_none());
    }

    #[test]
    fn iter() {
        let key = PendingKey::new_test_instance();
        let info = PendingInfo::new_test_instance();
        let fixture = Fixture::with_stored_data(vec![(key.clone(), info.clone())]);
        let tx = fixture.begin_read();

        let mut it = fixture.store.iter(&tx);
        let (k, v) = it.next().unwrap();
        assert_eq!(k, key);
        assert_eq!(v, info);
        assert!(it.next().is_none());
    }

    #[test]
    fn tracks_puts() {
        let fixture = Fixture::new();
        let mut txn = fixture.begin_write();
        let key = PendingKey::new_test_instance();
        let info = PendingInfo::new_test_instance();
        let put_tracker = fixture.store.track_puts();

        fixture.store.put(&mut txn, &key, &info);

        assert_eq!(put_tracker.output(), vec![(key, info)]);
    }

    #[test]
    fn tracks_deletions() {
        let fixture = Fixture::new();
        let mut txn = fixture.begin_write();
        let key = PendingKey::new_test_instance();
        let delete_tracker = fixture.store.track_deletions();

        fixture.store.del(&mut txn, &key);

        assert_eq!(delete_tracker.output(), vec![key]);
    }
}
