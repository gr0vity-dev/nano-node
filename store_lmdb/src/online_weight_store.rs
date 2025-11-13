use rsnano_nullable_lmdb::{DatabaseFlags, LmdbDatabase, LmdbEnvironment, WriteFlags};
use rsnano_types::Amount;
use store_traits::transaction::{
    LedgerReadTxn, LedgerReadTxnLmdbExt, LedgerWriteTxn, LedgerWriteTxnLmdbExt,
};

use crate::LmdbIterator;

pub struct LmdbOnlineWeightStore {
    database: LmdbDatabase,
}

impl LmdbOnlineWeightStore {
    pub fn new(env: &LmdbEnvironment) -> anyhow::Result<Self> {
        let database = env.create_db(Some("online_weight"), DatabaseFlags::empty())?;
        Ok(Self { database })
    }

    pub fn database(&self) -> LmdbDatabase {
        self.database
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, time: u64, amount: &Amount) {
        let time_bytes = time.to_be_bytes();
        let amount_bytes = amount.to_be_bytes();
        txn.put_lmdb(
            self.database,
            &time_bytes,
            &amount_bytes,
            WriteFlags::empty(),
        )
        .unwrap();
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, time: u64) {
        let time_bytes = time.to_be_bytes();
        txn.delete_lmdb(self.database, &time_bytes, None).unwrap();
    }

    pub fn iter<'txn>(
        &self,
        tx: &'txn dyn LedgerReadTxn,
    ) -> impl Iterator<Item = (u64, Amount)> + 'txn + use<'txn> {
        let cursor = tx.open_ro_cursor_lmdb(self.database).unwrap();

        LmdbIterator::new(cursor, |key, value| {
            let time = u64::from_be_bytes(key.try_into().unwrap());
            let amount = Amount::from_be_bytes(value.try_into().unwrap());
            (time, amount)
        })
    }

    /// Iterate in descending order
    pub fn iter_rev<'txn>(
        &self,
        tx: &'txn dyn LedgerReadTxn,
    ) -> impl Iterator<Item = (u64, Amount)> + 'txn + use<'txn> {
        let cursor = tx.open_ro_cursor_lmdb(self.database).unwrap();

        LmdbIterator::new_descending(cursor, |key, value| {
            let time = u64::from_be_bytes(key.try_into().unwrap());
            let amount = Amount::from_be_bytes(value.try_into().unwrap());
            (time, amount)
        })
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.count_lmdb(self.database)
    }

    pub fn clear(&self, txn: &mut dyn LedgerWriteTxn) {
        txn.clear_db_lmdb(self.database).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_nullable_lmdb::{DeleteEvent, PutEvent};
    use std::sync::Arc;

    struct Fixture {
        env: Arc<LmdbEnvironment>,
        store: LmdbOnlineWeightStore,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_stored_data(Vec::new())
        }

        fn with_stored_data(entries: Vec<(u64, Amount)>) -> Self {
            let mut env = LmdbEnvironment::null_builder()
                .database("online_weight", LmdbDatabase::new_null(42));

            for (key, value) in entries {
                env = env.entry(&key.to_be_bytes(), &value.to_be_bytes())
            }

            Self::with_env(env.build().build())
        }

        fn with_env(env: LmdbEnvironment) -> Self {
            let env = Arc::new(env);
            Self {
                store: LmdbOnlineWeightStore::new(&env).unwrap(),
                env,
            }
        }
    }

    #[test]
    fn empty_store() {
        let fixture = Fixture::new();
        let tx = fixture.env.begin_read();
        let store = &fixture.store;
        assert_eq!(store.count(&tx), 0);
        assert!(store.iter(&tx).next().is_none());
        assert!(store.iter_rev(&tx).next().is_none());
    }

    #[test]
    fn count() {
        let fixture = Fixture::with_stored_data(vec![(1, Amount::raw(100)), (2, Amount::raw(200))]);
        let txn = fixture.env.begin_read();

        let count = fixture.store.count(&txn);

        assert_eq!(count, 2);
    }

    #[test]
    fn add() {
        let fixture = Fixture::new();
        let mut txn = fixture.env.begin_write();
        let put_tracker = txn.track_puts();

        let time = 1;
        let amount = Amount::raw(2);
        fixture.store.put(&mut txn, time, &amount);

        assert_eq!(
            put_tracker.output(),
            vec![PutEvent {
                database: LmdbDatabase::new_null(42),
                key: time.to_be_bytes().to_vec(),
                value: amount.to_be_bytes().to_vec(),
                flags: WriteFlags::empty(),
            }]
        );
    }

    #[test]
    fn iterate_ascending() {
        let fixture = Fixture::with_stored_data(vec![(1, Amount::raw(100)), (2, Amount::raw(200))]);
        let txn = fixture.env.begin_read();

        let mut it = fixture.store.iter(&txn);
        assert_eq!(it.next(), Some((1, Amount::raw(100))));
        assert_eq!(it.next(), Some((2, Amount::raw(200))));
        assert_eq!(it.next(), None);
    }

    #[test]
    fn iterate_descending() {
        let fixture = Fixture::with_stored_data(vec![(1, Amount::raw(100)), (2, Amount::raw(200))]);
        let txn = fixture.env.begin_read();

        let mut it = fixture.store.iter_rev(&txn);
        assert_eq!(it.next(), Some((2, Amount::raw(200))));
        assert_eq!(it.next(), Some((1, Amount::raw(100))));
        assert_eq!(it.next(), None);
    }

    #[test]
    fn delete() {
        let fixture = Fixture::new();
        let mut txn = fixture.env.begin_write();
        let delete_tracker = txn.track_deletions();

        let time = 1;
        fixture.store.del(&mut txn, time);

        assert_eq!(
            delete_tracker.output(),
            vec![DeleteEvent {
                database: LmdbDatabase::new_null(42),
                key: time.to_be_bytes().to_vec()
            }]
        );
    }
}
