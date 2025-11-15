use std::sync::Arc;

use crate::store_utils::{store_database_from_lmdb, store_write_flags_from};
use rsnano_nullable_lmdb::{DatabaseFlags, LmdbDatabase, LmdbEnvironment, WriteFlags};
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::BlockHash;
use store_traits::{
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::{StoreDatabase, StoreErrorKind},
};

/// Stores the hash of the successor block for a given block hash
pub struct LmdbSuccessorStore {
    database: LmdbDatabase,
    put_listener: OutputListenerMt<(BlockHash, BlockHash)>,
}

impl LmdbSuccessorStore {
    pub fn new(env: &LmdbEnvironment) -> anyhow::Result<Self> {
        let database = env.create_db(Some(TABLE_NAME), DatabaseFlags::empty())?;
        Ok(Self {
            database,
            put_listener: OutputListenerMt::new(),
        })
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<(BlockHash, BlockHash)>> {
        self.put_listener.track()
    }

    fn store_database(&self) -> StoreDatabase {
        store_database_from_lmdb(self.database)
    }

    pub fn put(&self, tx: &mut dyn LedgerWriteTxn, block: &BlockHash, successor: &BlockHash) {
        if self.put_listener.is_tracked() {
            self.put_listener.emit((*block, *successor));
        }

        tx.put(
            self.store_database(),
            block.as_bytes(),
            successor.as_bytes(),
            store_write_flags_from(WriteFlags::empty()),
        )
        .unwrap();
    }

    pub fn del(&self, tx: &mut dyn LedgerWriteTxn, block: &BlockHash) {
        tx.delete(self.store_database(), block.as_bytes(), None)
            .unwrap();
    }

    pub fn get(&self, tx: &dyn LedgerReadTxn, block: &BlockHash) -> Option<BlockHash> {
        match tx.get(self.store_database(), block.as_bytes()) {
            Ok(bytes) => BlockHash::from_slice(bytes),
            Err(e) if e.is_not_found() => None,
            Err(e) => match e.kind() {
                StoreErrorKind::PageNotFound => {
                    panic!("Could not load successor hash: PageNotFound")
                }
                _ => panic!("Could not load successor hash: {}", e),
            },
        }
    }

    pub fn count(&self, tx: &dyn LedgerReadTxn) -> u64 {
        tx.count(self.store_database())
    }
}

const TABLE_NAME: &str = "successors";

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_nullable_lmdb::{DeleteEvent, Error, PutEvent};
    use std::sync::Arc;

    #[test]
    fn initialize() {
        let fixture = Fixture::with_entries(&[]);
        assert_eq!(fixture.store.database, TEST_DATABASE);
    }

    #[test]
    fn count() {
        let fixture = Fixture::with_entries(&[
            (1.into(), 2.into()),
            (3.into(), 4.into()),
            (5.into(), 6.into()),
        ]);
        let tx = fixture.begin_read_txn();
        assert_eq!(fixture.store.count(&tx), 3);
    }

    #[test]
    fn put() {
        let fixture = Fixture::with_entries(&[]);
        let mut tx = fixture.begin_write_txn();
        let put_tracker = tx.as_inner_mut().track_puts();
        let block = BlockHash::from(1);
        let successor = BlockHash::from(2);

        fixture.store.put(&mut tx, &block, &successor);

        assert_eq!(
            put_tracker.output(),
            vec![PutEvent {
                database: TEST_DATABASE,
                key: block.as_bytes().to_vec(),
                value: successor.as_bytes().to_vec(),
                flags: WriteFlags::empty()
            }]
        );
    }

    #[test]
    fn track_puts() {
        let fixture = Fixture::with_entries(&[]);
        let put_tracker = fixture.store.track_puts();
        let mut tx = fixture.begin_write_txn();
        let block = BlockHash::from(1);
        let successor = BlockHash::from(2);

        fixture.store.put(&mut tx, &block, &successor);

        assert_eq!(put_tracker.output(), vec![(block, successor)]);
    }

    #[test]
    fn get() {
        let fixture = Fixture::with_entries(&[
            (1.into(), 2.into()),
            (3.into(), 4.into()),
            (5.into(), 6.into()),
        ]);

        let tx = fixture.begin_read_txn();
        let successor = fixture.store.get(&tx, &3.into());
        assert_eq!(successor, Some(4.into()))
    }

    #[test]
    fn no_successor_found() {
        let fixture = Fixture::with_entries(&[]);

        let tx = fixture.begin_read_txn();
        let successor = fixture.store.get(&tx, &3.into());
        assert_eq!(successor, None);
    }

    #[test]
    #[should_panic = "Could not load successor hash: PageNotFound"]
    fn get_unexpected_error() {
        let block_hash = BlockHash::from(1);
        let env = LmdbEnvironment::null_builder()
            .database(TABLE_NAME, TEST_DATABASE)
            .error(block_hash.as_bytes(), Error::PageNotFound)
            .build()
            .build();
        let fixture = Fixture::with_env(env);
        let tx = fixture.begin_read_txn();
        fixture.store.get(&tx, &block_hash);
    }

    #[test]
    fn delete() {
        let fixture = Fixture::with_entries(&[]);
        let mut tx = fixture.begin_write_txn();
        let delete_tracker = tx.as_inner_mut().track_deletions();

        let block_hash = BlockHash::from(123);
        fixture.store.del(&mut tx, &block_hash);

        assert_eq!(
            delete_tracker.output(),
            vec![DeleteEvent {
                database: TEST_DATABASE,
                key: block_hash.as_bytes().to_vec()
            }]
        );
    }

    const TEST_DATABASE: LmdbDatabase = LmdbDatabase::new_null(42);

    struct Fixture {
        env: Arc<LmdbEnvironment>,
        store: LmdbSuccessorStore,
    }

    impl Fixture {
        fn with_entries(entries: &[(BlockHash, BlockHash)]) -> Self {
            let mut builder = LmdbEnvironment::null_builder().database(TABLE_NAME, TEST_DATABASE);

            for (block_hash, successor) in entries {
                builder = builder.entry(block_hash.as_bytes(), successor.as_bytes());
            }

            Self::with_env(builder.build().build())
        }

        fn with_env(env: LmdbEnvironment) -> Self {
            let env = Arc::new(env);
            let store = LmdbSuccessorStore::new(&env).unwrap();
            Self { env, store }
        }

        fn begin_read_txn(&self) -> crate::transaction::LmdbLedgerReadTxn {
            crate::transaction::LmdbLedgerReadTxn::new(self.env.begin_read())
        }

        fn begin_write_txn(&self) -> crate::transaction::LmdbLedgerWriteTxn {
            crate::transaction::LmdbLedgerWriteTxn::new(self.env.begin_write())
        }
    }
}
