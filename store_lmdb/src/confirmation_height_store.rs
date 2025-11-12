use std::ops::RangeBounds;

use rsnano_nullable_lmdb::{
    ConfiguredDatabase, DatabaseFlags, Error, LmdbDatabase, LmdbEnvironment, WriteFlags,
};
use rsnano_types::{Account, ConfirmationHeightInfo};
use store_traits::transaction::{LedgerReadTxn, LedgerWriteTxn};

use crate::{
    CONFIRMATION_HEIGHT_TEST_DATABASE, LmdbIterator, LmdbRangeIterator, parallel_traversal,
};

pub struct LmdbConfirmationHeightStore {
    database: LmdbDatabase,
}

impl LmdbConfirmationHeightStore {
    pub fn new(env: &LmdbEnvironment) -> anyhow::Result<Self> {
        let database = env.create_db(Some("confirmation_height"), DatabaseFlags::empty())?;

        Ok(Self { database })
    }

    pub fn database(&self) -> LmdbDatabase {
        self.database
    }

    pub fn put(
        &self,
        txn: &mut dyn LedgerWriteTxn,
        account: &Account,
        info: &ConfirmationHeightInfo,
    ) {
        txn.raw_put(
            self.database,
            account.as_bytes(),
            &info.to_bytes(),
            WriteFlags::empty(),
        )
        .unwrap();
    }

    pub fn get(&self, txn: &dyn LedgerReadTxn, account: &Account) -> Option<ConfirmationHeightInfo> {
        match txn.raw_get(self.database, account.as_bytes()) {
            Err(Error::NotFound) => None,
            Ok(mut bytes) => Some(
                ConfirmationHeightInfo::deserialize(&mut bytes)
                    .expect("Should be valid conf height data"),
            ),
            Err(e) => {
                panic!("Could not load confirmation height info: {:?}", e);
            }
        }
    }

    pub fn exists(&self, txn: &dyn LedgerReadTxn, account: &Account) -> bool {
        txn.raw_exists(self.database, account.as_bytes())
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, account: &Account) {
        txn.raw_delete(self.database, account.as_bytes(), None).unwrap();
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.raw_count(self.database)
    }

    pub fn clear(&self, txn: &mut dyn LedgerWriteTxn) {
        txn.raw_clear_db(self.database).unwrap()
    }

    pub fn iter<'tx>(
        &self,
        tx: &'tx dyn LedgerReadTxn,
    ) -> impl Iterator<Item = (Account, ConfirmationHeightInfo)> + 'tx + use<'tx> {
        let cursor = tx.raw_open_ro_cursor(self.database).unwrap();
        LmdbIterator::new(cursor, read_conf_height_record)
    }

    pub fn iter_range<'txn>(
        &self,
        tx: &'txn dyn LedgerReadTxn,
        range: impl RangeBounds<Account> + 'static,
    ) -> impl Iterator<Item = (Account, ConfirmationHeightInfo)> + 'txn {
        let cursor = tx.raw_open_ro_cursor(self.database).unwrap();
        LmdbRangeIterator::new(
            cursor,
            range.start_bound().map(|b| b.as_bytes().to_vec()),
            range.end_bound().map(|b| b.as_bytes().to_vec()),
            read_conf_height_record,
        )
    }

    pub fn for_each_par(
        &self,
        env: &LmdbEnvironment,
        thread_count: usize,
        action: impl Fn(&mut dyn Iterator<Item = (Account, ConfirmationHeightInfo)>) + Send + Sync,
    ) {
        parallel_traversal(thread_count, &|start, end, is_last| {
            let txn = env.begin_read();
            let start_account = Account::from(start);
            let end_account = Account::from(end);
            if is_last {
                let mut iter = self.iter_range(&txn, start_account..);
                action(&mut iter);
            } else {
                let mut iter = self.iter_range(&txn, start_account..end_account);
                action(&mut iter);
            }
            txn.commit();
        })
    }
}

pub struct ConfiguredConfirmationHeightDatabaseBuilder {
    database: ConfiguredDatabase,
}

impl ConfiguredConfirmationHeightDatabaseBuilder {
    pub fn new() -> Self {
        Self {
            database: ConfiguredDatabase::new(
                CONFIRMATION_HEIGHT_TEST_DATABASE,
                "confirmation_height",
            ),
        }
    }

    pub fn height(mut self, account: &Account, info: &ConfirmationHeightInfo) -> Self {
        self.database.insert(account.as_bytes(), info.to_bytes());
        self
    }

    pub fn build(self) -> ConfiguredDatabase {
        self.database
    }

    pub fn create(hashes: Vec<(Account, ConfirmationHeightInfo)>) -> ConfiguredDatabase {
        let mut builder = Self::new();
        for (account, info) in hashes {
            builder = builder.height(&account, &info);
        }
        builder.build()
    }
}

fn read_conf_height_record(key: &[u8], mut value: &[u8]) -> (Account, ConfirmationHeightInfo) {
    let account = Account::from_bytes(key.try_into().unwrap());
    let info = ConfirmationHeightInfo::deserialize(&mut value).unwrap();
    (account, info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_nullable_lmdb::PutEvent;
    use rsnano_types::BlockHash;
    use std::sync::Arc;

    struct Fixture {
        env: Arc<LmdbEnvironment>,
        store: LmdbConfirmationHeightStore,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_env(LmdbEnvironment::new_null())
        }

        fn with_env(env: LmdbEnvironment) -> Self {
            let env = Arc::new(env);
            Self {
                store: LmdbConfirmationHeightStore::new(&env).unwrap(),
                env,
            }
        }
    }

    #[test]
    fn empty_store() {
        let fixture = Fixture::new();
        let store = &fixture.store;
        let txn = fixture.env.begin_read();
        assert!(store.get(&txn, &Account::from(0)).is_none());
        assert_eq!(store.exists(&txn, &Account::from(0)), false);
        assert!(store.iter(&txn).next().is_none());
        assert!(store.iter_range(&txn, Account::from(0)..).next().is_none());
    }

    #[test]
    fn add_account() {
        let fixture = Fixture::new();
        let mut txn = fixture.env.begin_write();
        let put_tracker = txn.track_puts();

        let account = Account::from(1);
        let info = ConfirmationHeightInfo::new(1, BlockHash::from(2));
        fixture.store.put(&mut txn, &account, &info);

        assert_eq!(
            put_tracker.output(),
            vec![PutEvent {
                database: LmdbDatabase::new_null(42),
                key: account.as_bytes().to_vec(),
                value: info.to_bytes().to_vec(),
                flags: WriteFlags::empty(),
            }]
        )
    }

    #[test]
    fn load() {
        let account = Account::from(1);
        let info = ConfirmationHeightInfo::new(1, BlockHash::from(2));

        let env = LmdbEnvironment::null_builder()
            .database("confirmation_height", LmdbDatabase::new_null(100))
            .entry(account.as_bytes(), &info.to_bytes())
            .build()
            .build();

        let fixture = Fixture::with_env(env);
        let txn = fixture.env.begin_read();
        let result = fixture.store.get(&txn, &account);

        assert_eq!(result, Some(info))
    }

    #[test]
    fn iterate_one_account() -> anyhow::Result<()> {
        let account = Account::from(1);
        let info = ConfirmationHeightInfo::new(1, BlockHash::from(2));

        let env = LmdbEnvironment::null_builder()
            .database("confirmation_height", LmdbDatabase::new_null(100))
            .entry(account.as_bytes(), &info.to_bytes())
            .build()
            .build();

        let fixture = Fixture::with_env(env);
        let txn = fixture.env.begin_read();
        let mut it = fixture.store.iter(&txn);
        assert_eq!(it.next(), Some((account, info)));
        assert!(it.next().is_none());
        Ok(())
    }

    #[test]
    fn clear() {
        let fixture = Fixture::new();
        let mut txn = fixture.env.begin_write();
        let clear_tracker = txn.track_clears();

        fixture.store.clear(&mut txn);

        assert_eq!(clear_tracker.output(), vec![LmdbDatabase::new_null(42)])
    }
}
