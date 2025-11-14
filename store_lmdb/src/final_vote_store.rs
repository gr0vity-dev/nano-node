use std::ops::RangeBounds;

use rsnano_nullable_lmdb::{DatabaseFlags, LmdbDatabase, LmdbEnvironment, WriteFlags};
use rsnano_types::{BlockHash, QualifiedRoot};
use store_traits::{
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::StoreDatabase,
};

use crate::{
    LmdbIterator, LmdbRangeIterator,
    store_utils::{
        lmdb_ro_cursor_from_store, store_database_from_lmdb, store_write_flags_from,
    },
};

/// Maps root to block hash for generated final votes.
/// nano::qualified_root -> nano::block_hash
pub struct LmdbFinalVoteStore {
    database: LmdbDatabase,
}

impl LmdbFinalVoteStore {
    pub fn new(env: &LmdbEnvironment) -> anyhow::Result<Self> {
        let database = env.create_db(Some("final_votes"), DatabaseFlags::empty())?;

        Ok(Self { database })
    }

    pub fn database(&self) -> LmdbDatabase {
        self.database
    }

    fn store_database(&self) -> StoreDatabase {
        store_database_from_lmdb(self.database)
    }

    /// Returns *true* if root + hash was inserted or the same root/hash pair was already in the database
    pub fn put(
        &self,
        txn: &mut dyn LedgerWriteTxn,
        root: &QualifiedRoot,
        hash: &BlockHash,
    ) -> bool {
        let root_bytes = root.to_bytes();
        match txn.get(self.store_database(), &root_bytes) {
            Err(e) if e.is_not_found() => {
                txn.put(
                    self.store_database(),
                    &root_bytes,
                    hash.as_bytes(),
                    store_write_flags_from(WriteFlags::empty()),
                )
                .unwrap();
                true
            }
            Ok(bytes) => BlockHash::from_slice(bytes).unwrap() == *hash,
            Err(e) => {
                panic!("Could not get final vote: {:?}", e);
            }
        }
    }

    pub fn iter<'tx>(
        &self,
        tx: &'tx dyn LedgerReadTxn,
    ) -> impl Iterator<Item = (QualifiedRoot, BlockHash)> + 'tx + use<'tx> {
        let cursor = tx.open_ro_cursor(self.store_database()).unwrap();
        let cursor = lmdb_ro_cursor_from_store(cursor);
        LmdbIterator::new(cursor, read_final_vote_record)
    }

    pub fn iter_range<'tx>(
        &self,
        tx: &'tx dyn LedgerReadTxn,
        range: impl RangeBounds<QualifiedRoot> + 'static,
    ) -> impl Iterator<Item = (QualifiedRoot, BlockHash)> + 'tx {
        let cursor = tx.open_ro_cursor(self.store_database()).unwrap();
        let cursor = lmdb_ro_cursor_from_store(cursor);
        LmdbRangeIterator::new(
            cursor,
            range.start_bound().map(|b| b.to_bytes().to_vec()),
            range.end_bound().map(|b| b.to_bytes().to_vec()),
            read_final_vote_record,
        )
    }

    pub fn get(&self, tx: &dyn LedgerReadTxn, root: &QualifiedRoot) -> Option<BlockHash> {
        let result = tx.get(self.store_database(), &root.to_bytes());
        match result {
            Err(e) if e.is_not_found() => None,
            Ok(mut bytes) => {
                Some(BlockHash::deserialize(&mut bytes).expect("Should be valid block hash data"))
            }
            Err(e) => panic!("Could not load final vote info {:?}", e),
        }
    }

    pub fn del(&self, tx: &mut dyn LedgerWriteTxn, root: &QualifiedRoot) {
        let root_bytes = root.to_bytes();
        tx.delete(self.store_database(), &root_bytes, None).unwrap();
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.count(self.store_database())
    }

    pub fn clear(&self, txn: &mut dyn LedgerWriteTxn) {
        txn.clear_db(self.store_database()).unwrap();
    }
}

fn read_final_vote_record(mut key: &[u8], mut value: &[u8]) -> (QualifiedRoot, BlockHash) {
    let root = QualifiedRoot::deserialize(&mut key).unwrap();
    let hash = BlockHash::deserialize(&mut value).unwrap();
    (root, hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_nullable_lmdb::DeleteEvent;
    use std::sync::Arc;
    use crate::transaction::{LmdbLedgerReadTxn, LmdbLedgerWriteTxn};

    const TEST_DATABASE: LmdbDatabase = LmdbDatabase::new_null(100);

    struct Fixture {
        env: Arc<LmdbEnvironment>,
        store: LmdbFinalVoteStore,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_stored_entries(Vec::new())
        }

        fn with_stored_entries(entries: Vec<(QualifiedRoot, BlockHash)>) -> Self {
            let mut env = LmdbEnvironment::null_builder().database("final_votes", TEST_DATABASE);
            for (key, value) in entries {
                env = env.entry(&key.to_bytes(), value.as_bytes());
            }
            Self::with_env(env.build().build())
        }

        fn with_env(env: LmdbEnvironment) -> Self {
            let env = Arc::new(env);
            Self {
                store: LmdbFinalVoteStore::new(&env).unwrap(),
                env,
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
    fn load() {
        let root = QualifiedRoot::new_test_instance();
        let hash = BlockHash::from(333);
        let fixture = Fixture::with_stored_entries(vec![(root.clone(), hash)]);
        let txn = fixture.begin_read();

        let result = fixture.store.get(&txn, &root);

        assert_eq!(result, Some(hash));
    }

    #[test]
    fn delete() {
        let root = QualifiedRoot::new_test_instance();
        let fixture = Fixture::with_stored_entries(vec![(root.clone(), BlockHash::from(333))]);
        let mut txn = fixture.begin_write();
        let delete_tracker = txn.as_inner_mut().track_deletions();

        fixture.store.del(&mut txn, &root);

        assert_eq!(
            delete_tracker.output(),
            vec![DeleteEvent {
                key: root.to_bytes().to_vec(),
                database: TEST_DATABASE.into(),
            }]
        )
    }

    #[test]
    fn clear() {
        let fixture = Fixture::new();
        let mut txn = fixture.begin_write();
        let clear_tracker = txn.as_inner_mut().track_clears();

        fixture.store.clear(&mut txn);

        assert_eq!(clear_tracker.output(), vec![TEST_DATABASE.into()]);
    }
}
