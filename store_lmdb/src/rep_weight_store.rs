use std::sync::Arc;

use rsnano_nullable_lmdb::{
    ConfiguredDatabase, DatabaseFlags, LmdbDatabase, LmdbEnvironment, RoCursor, WriteFlags,
    sys::{MDB_FIRST, MDB_NEXT, MDB_cursor_op},
};
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::{Amount, PublicKey};
use store_traits::{
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::StoreDatabase,
};

use crate::{
    REP_WEIGHT_TEST_DATABASE,
    store_utils::{
        lmdb_ro_cursor_from_store, store_database_from_lmdb, store_write_flags_from,
    },
};

pub struct LmdbRepWeightStore {
    database: LmdbDatabase,
    delete_listener: OutputListenerMt<PublicKey>,
    put_listener: OutputListenerMt<(PublicKey, Amount)>,
}

impl LmdbRepWeightStore {
    pub fn new(env: &LmdbEnvironment) -> anyhow::Result<Self> {
        let database = env.create_db(Some("rep_weights"), DatabaseFlags::empty())?;

        Ok(Self {
            database,
            delete_listener: OutputListenerMt::new(),
            put_listener: OutputListenerMt::new(),
        })
    }

    pub fn track_deletions(&self) -> Arc<OutputTrackerMt<PublicKey>> {
        self.delete_listener.track()
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<(PublicKey, Amount)>> {
        self.put_listener.track()
    }

    fn store_database(&self) -> StoreDatabase {
        store_database_from_lmdb(self.database)
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn, pub_key: &PublicKey) -> Option<Amount> {
        match txn.get(self.store_database(), pub_key.as_bytes()) {
            Ok(mut bytes) => Some(Amount::deserialize(&mut bytes).expect("Should be valid amount")),
            Err(e) if e.is_not_found() => None,
            Err(e) => {
                panic!("Could not load rep_weight: {:?}", e);
            }
        }
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, representative: PublicKey, weight: Amount) {
        self.put_listener.emit((representative, weight));

        txn.put(
            self.store_database(),
            representative.as_bytes(),
            &weight.to_be_bytes(),
            store_write_flags_from(WriteFlags::empty()),
        )
        .unwrap();
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, representative: &PublicKey) {
        self.delete_listener.emit(*representative);

        txn.delete(self.store_database(), representative.as_bytes(), None)
            .unwrap();
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.count(self.store_database())
    }

    pub fn iter<'a>(&self, txn: &'a dyn LedgerReadTxn) -> RepWeightIterator<'a> {
        let cursor = txn.open_ro_cursor(self.store_database()).unwrap();
        let cursor = lmdb_ro_cursor_from_store(cursor);
        RepWeightIterator {
            cursor,
            operation: MDB_FIRST,
        }
    }
}

pub struct RepWeightIterator<'txn> {
    cursor: RoCursor<'txn>,
    operation: MDB_cursor_op,
}

impl<'txn> Iterator for RepWeightIterator<'txn> {
    type Item = (PublicKey, Amount);

    fn next(&mut self) -> Option<Self::Item> {
        match self.cursor.get(None, None, self.operation) {
            Err(rsnano_nullable_lmdb::Error::NotFound) => None,
            Ok((Some(k), v)) => {
                self.operation = MDB_NEXT;
                Some((
                    PublicKey::from_slice(k).unwrap(),
                    Amount::from_be_bytes(v.try_into().unwrap()),
                ))
            }
            Ok(_) => unreachable!(),
            Err(_) => unreachable!(),
        }
    }
}

pub struct ConfiguredRepWeightDatabaseBuilder {
    database: ConfiguredDatabase,
}

impl ConfiguredRepWeightDatabaseBuilder {
    pub fn new() -> Self {
        Self {
            database: ConfiguredDatabase::new(REP_WEIGHT_TEST_DATABASE, "rep_weights"),
        }
    }

    pub fn entry(mut self, account: PublicKey, weight: Amount) -> Self {
        self.database
            .insert(account.as_bytes(), weight.to_be_bytes());
        self
    }

    pub fn build(self) -> ConfiguredDatabase {
        self.database
    }

    pub fn create(hashes: Vec<(PublicKey, Amount)>) -> ConfiguredDatabase {
        let mut builder = Self::new();
        for (account, weight) in hashes {
            builder = builder.entry(account, weight);
        }
        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_nullable_lmdb::{DeleteEvent, PutEvent, WriteFlags};

    #[test]
    fn count() {
        let fixture =
            Fixture::with_stored_data(vec![(1.into(), 100.into()), (2.into(), 200.into())]);
        let txn = fixture.begin_read_txn();

        assert_eq!(fixture.store.count(&txn), 2);
    }

    #[test]
    fn put() {
        let fixture = Fixture::new();
        let mut txn = fixture.begin_write_txn();
        let put_tracker = txn.as_inner_mut().track_puts();
        let account = PublicKey::from(1);
        let weight = Amount::from(42);

        fixture.store.put(&mut txn, account, weight);

        assert_eq!(
            put_tracker.output(),
            vec![PutEvent {
                database: REP_WEIGHT_TEST_DATABASE.into(),
                key: account.as_bytes().to_vec(),
                value: weight.to_be_bytes().to_vec(),
                flags: WriteFlags::empty()
            }]
        );
    }

    #[test]
    fn load_weight() {
        let account = PublicKey::from(1);
        let weight = Amount::from(42);
        let fixture = Fixture::with_stored_data(vec![(account, weight)]);
        let txn = fixture.begin_read_txn();

        let result = fixture.store.get(&txn, &account);

        assert_eq!(result, Some(weight));
    }

    #[test]
    fn delete() {
        let fixture = Fixture::new();
        let mut txn = fixture.begin_write_txn();
        let delete_tracker = txn.as_inner_mut().track_deletions();
        let account = PublicKey::from(1);

        fixture.store.del(&mut txn, &account);

        assert_eq!(
            delete_tracker.output(),
            vec![DeleteEvent {
                database: REP_WEIGHT_TEST_DATABASE.into(),
                key: account.as_bytes().to_vec()
            }]
        )
    }

    #[test]
    fn iter_empty() {
        let fixture = Fixture::new();
        let txn = fixture.begin_read_txn();
        let mut iter = fixture.store.iter(&txn);
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn iter() {
        let account1 = PublicKey::from(1);
        let account2 = PublicKey::from(2);
        let weight1 = Amount::from(100);
        let weight2 = Amount::from(200);
        let fixture = Fixture::with_stored_data(vec![(account1, weight1), (account2, weight2)]);

        let txn = fixture.begin_read_txn();
        let mut iter = fixture.store.iter(&txn);
        assert_eq!(iter.next(), Some((account1, weight1)));
        assert_eq!(iter.next(), Some((account2, weight2)));
        assert_eq!(iter.next(), None);
    }

    struct Fixture {
        env: Arc<LmdbEnvironment>,
        store: LmdbRepWeightStore,
    }

    impl Fixture {
        pub fn new() -> Self {
            Self::with_stored_data(Vec::new())
        }

        pub fn with_stored_data(entries: Vec<(PublicKey, Amount)>) -> Self {
            let env = LmdbEnvironment::null_builder()
                .configured_database(ConfiguredRepWeightDatabaseBuilder::create(entries))
                .build();
            let env = Arc::new(env);
            Self {
                store: LmdbRepWeightStore::new(&env).unwrap(),
                env,
            }
        }

        fn begin_read_txn(&self) -> crate::transaction::LmdbLedgerReadTxn {
            crate::transaction::LmdbLedgerReadTxn::new(self.env.begin_read())
        }

        fn begin_write_txn(&self) -> crate::transaction::LmdbLedgerWriteTxn {
            crate::transaction::LmdbLedgerWriteTxn::new(self.env.begin_write())
        }
    }
}
