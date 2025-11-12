use std::ops::RangeBounds;

use rsnano_nullable_lmdb::{
    DatabaseFlags, Error, LmdbDatabase, LmdbEnvironment, WriteFlags,
};
use rsnano_types::{BlockHash, QualifiedRoot};
use store_traits::transaction::{LedgerReadTxn, LedgerWriteTxn};

use crate::{LmdbIterator, LmdbRangeIterator};

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

    /// Returns *true* if root + hash was inserted or the same root/hash pair was already in the database
    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, root: &QualifiedRoot, hash: &BlockHash) -> bool {
        let root_bytes = root.to_bytes();
        match txn.raw_get(self.database, &root_bytes) {
            Err(Error::NotFound) => {
                txn.raw_put(
                    self.database,
                    &root_bytes,
                    hash.as_bytes(),
                    WriteFlags::empty(),
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
        let cursor = tx.raw_open_ro_cursor(self.database).unwrap();
        LmdbIterator::new(cursor, read_final_vote_record)
    }

    pub fn iter_range<'tx>(
        &self,
        tx: &'tx dyn LedgerReadTxn,
        range: impl RangeBounds<QualifiedRoot> + 'static,
    ) -> impl Iterator<Item = (QualifiedRoot, BlockHash)> + 'tx {
        let cursor = tx.raw_open_ro_cursor(self.database).unwrap();
        LmdbRangeIterator::new(
            cursor,
            range.start_bound().map(|b| b.to_bytes().to_vec()),
            range.end_bound().map(|b| b.to_bytes().to_vec()),
            read_final_vote_record,
        )
    }

    pub fn get(&self, tx: &dyn LedgerReadTxn, root: &QualifiedRoot) -> Option<BlockHash> {
        let result = tx.raw_get(self.database, &root.to_bytes());
        match result {
            Err(Error::NotFound) => None,
            Ok(mut bytes) => {
                Some(BlockHash::deserialize(&mut bytes).expect("Should be valid block hash data"))
            }
            Err(e) => panic!("Could not load final vote info {:?}", e),
        }
    }

    pub fn del(&self, tx: &mut dyn LedgerWriteTxn, root: &QualifiedRoot) {
        let root_bytes = root.to_bytes();
        tx.raw_delete(self.database, &root_bytes, None).unwrap();
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.raw_count(self.database)
    }

    pub fn clear(&self, txn: &mut dyn LedgerWriteTxn) {
        txn.raw_clear_db(self.database).unwrap();
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
    }

    #[test]
    fn load() {
        let root = QualifiedRoot::new_test_instance();
        let hash = BlockHash::from(333);
        let fixture = Fixture::with_stored_entries(vec![(root.clone(), hash)]);
        let txn = fixture.env.begin_read();

        let result = fixture.store.get(&txn, &root);

        assert_eq!(result, Some(hash));
    }

    #[test]
    fn delete() {
        let root = QualifiedRoot::new_test_instance();
        let fixture = Fixture::with_stored_entries(vec![(root.clone(), BlockHash::from(333))]);
        let mut txn = fixture.env.begin_write();
        let delete_tracker = txn.track_deletions();

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
        let mut txn = fixture.env.begin_write();
        let clear_tracker = txn.track_clears();

        fixture.store.clear(&mut txn);

        assert_eq!(clear_tracker.output(), vec![TEST_DATABASE.into()]);
    }
}
