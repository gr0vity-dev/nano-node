use std::{ops::RangeBounds, sync::Arc};

use rsnano_nullable_lmdb::{
    ConfiguredDatabase, DatabaseFlags, LmdbDatabase, LmdbEnvironment, WriteFlags,
};
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::{Account, AccountInfo};
use store_traits::{
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::StoreDatabase,
};

use crate::{
    ACCOUNT_TEST_DATABASE,
    iterator::{LmdbIterator, LmdbRangeIterator},
    parallel_traversal,
    store_utils::{lmdb_ro_cursor_from_store, store_database_from_lmdb, store_write_flags_from},
    transaction::LmdbLedgerReadTxn,
};

pub struct LmdbAccountStore {
    /// U256 (arbitrary key) -> blob
    database: LmdbDatabase,
    put_listener: OutputListenerMt<(Account, AccountInfo)>,
}

impl LmdbAccountStore {
    pub fn new(env: &LmdbEnvironment) -> anyhow::Result<Self> {
        let database = env.create_db(Some("accounts"), DatabaseFlags::empty())?;

        Ok(Self {
            database,
            put_listener: OutputListenerMt::new(),
        })
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<(Account, AccountInfo)>> {
        self.put_listener.track()
    }

    pub fn database(&self) -> LmdbDatabase {
        self.database
    }

    fn store_database(&self) -> StoreDatabase {
        store_database_from_lmdb(self.database)
    }

    pub fn put(&self, transaction: &mut dyn LedgerWriteTxn, account: &Account, info: &AccountInfo) {
        if self.put_listener.is_tracked() {
            self.put_listener.emit((*account, info.clone()));
        }
        transaction
            .put(
                self.store_database(),
                account.as_bytes(),
                &info.to_bytes(),
                store_write_flags_from(WriteFlags::empty()),
            )
            .unwrap();
    }

    pub fn get(&self, transaction: &dyn LedgerReadTxn, account: &Account) -> Option<AccountInfo> {
        let result = transaction.get(self.store_database(), account.as_bytes());
        match result {
            Err(e) if e.is_not_found() => None,
            Ok(mut bytes) => AccountInfo::deserialize(&mut bytes).ok(),
            Err(e) => panic!("Could not load account info {:?}", e),
        }
    }

    pub fn del(&self, transaction: &mut dyn LedgerWriteTxn, account: &Account) {
        transaction
            .delete(self.store_database(), account.as_bytes(), None)
            .unwrap();
    }

    pub fn iter<'txn>(
        &self,
        tx: &'txn dyn LedgerReadTxn,
    ) -> impl Iterator<Item = (Account, AccountInfo)> + 'txn + use<'txn> {
        let cursor = tx
            .open_ro_cursor(self.store_database())
            .expect("could not read from account store");
        let cursor = lmdb_ro_cursor_from_store(cursor);

        LmdbIterator::new(cursor, read_account_info_record)
    }

    pub fn iter_range<'txn>(
        &self,
        tx: &'txn dyn LedgerReadTxn,
        range: impl RangeBounds<Account> + 'static,
    ) -> Box<dyn Iterator<Item = (Account, AccountInfo)> + 'txn> {
        let cursor = tx.open_ro_cursor(self.store_database()).unwrap();
        let cursor = lmdb_ro_cursor_from_store(cursor);
        let start = range.start_bound().map(|b| b.as_bytes().to_vec());
        let end = range.end_bound().map(|b| b.as_bytes().to_vec());
        Box::new(LmdbRangeIterator::new(
            cursor,
            start,
            end,
            read_account_info_record,
        ))
    }

    pub fn for_each_par(
        &self,
        env: &LmdbEnvironment,
        thread_count: usize,
        action: impl Fn(&mut dyn Iterator<Item = (Account, AccountInfo)>) + Send + Sync,
    ) {
        parallel_traversal(thread_count, &|start, end, is_last| {
            let txn = LmdbLedgerReadTxn::new(env.begin_read());
            let start_account = Account::from(start);
            let end_account = Account::from(end);
            if is_last {
                let mut iter = self.iter_range(&txn, start_account..);
                action(&mut iter);
            } else {
                let mut iter = self.iter_range(&txn, start_account..end_account);
                action(&mut iter);
            }
            txn.into_inner().commit();
        })
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.raw_count(self.store_database())
    }
}

pub struct ConfiguredAccountDatabaseBuilder {
    database: ConfiguredDatabase,
}

impl ConfiguredAccountDatabaseBuilder {
    pub fn new() -> Self {
        Self {
            database: ConfiguredDatabase::new(ACCOUNT_TEST_DATABASE, "accounts"),
        }
    }

    pub fn account(mut self, account: &Account, info: &AccountInfo) -> Self {
        self.database.insert(account.as_bytes(), info.to_bytes());
        self
    }

    pub fn build(self) -> ConfiguredDatabase {
        self.database
    }

    pub fn create(frontiers: Vec<(Account, AccountInfo)>) -> ConfiguredDatabase {
        let mut builder = Self::new();
        for (account, info) in frontiers {
            builder = builder.account(&account, &info);
        }
        builder.build()
    }
}

fn read_account_info_record(key: &[u8], mut value: &[u8]) -> (Account, AccountInfo) {
    let account = Account::from_bytes(key.try_into().unwrap());
    let info = AccountInfo::deserialize(&mut value).unwrap();
    (account, info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::{LmdbLedgerReadTxn, LmdbLedgerWriteTxn};
    use rsnano_nullable_lmdb::{DeleteEvent, PutEvent};
    use rsnano_types::{Amount, BlockHash};
    use std::sync::Mutex;

    struct Fixture {
        env: Arc<LmdbEnvironment>,
        store: LmdbAccountStore,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_stored_accounts(Vec::new())
        }

        fn with_stored_accounts(accounts: Vec<(Account, AccountInfo)>) -> Self {
            let env = LmdbEnvironment::null_builder()
                .configured_database(ConfiguredAccountDatabaseBuilder::create(accounts))
                .build();
            Self::with_env(env)
        }

        fn with_env(env: LmdbEnvironment) -> Self {
            let env = Arc::new(env);
            let store = LmdbAccountStore::new(&env).unwrap();

            Fixture { env, store }
        }

        fn begin_read_txn(&self) -> LmdbLedgerReadTxn {
            LmdbLedgerReadTxn::new(self.env.begin_read())
        }

        fn begin_write_txn(&self) -> LmdbLedgerWriteTxn {
            LmdbLedgerWriteTxn::new(self.env.begin_write())
        }
    }

    #[test]
    fn empty_store() {
        let fixture = Fixture::new();
        let txn = fixture.begin_read_txn();
        let account = Account::from(1);
        let result = fixture.store.get(&txn, &account);
        assert_eq!(result, None);
        assert_eq!(fixture.store.count(&txn), 0);
    }

    #[test]
    fn add_one_account() {
        let fixture = Fixture::new();
        let mut txn = fixture.begin_write_txn();
        let put_tracker = txn.as_inner_mut().track_puts();

        let account = Account::from(1);
        let info = AccountInfo::new_test_instance();
        fixture.store.put(&mut txn, &account, &info);

        assert_eq!(
            put_tracker.output(),
            vec![PutEvent {
                database: ACCOUNT_TEST_DATABASE.into(),
                key: account.as_bytes().to_vec(),
                value: info.to_bytes().to_vec(),
                flags: WriteFlags::empty()
            }]
        );
    }

    #[test]
    fn load_account() {
        let account = Account::from(1);
        let info = AccountInfo::new_test_instance();
        let fixture = Fixture::with_stored_accounts(vec![(account.clone(), info.clone())]);
        let txn = fixture.begin_read_txn();

        let result = fixture.store.get(&txn, &account);

        assert_eq!(result, Some(info));
    }

    #[test]
    fn count() {
        let fixture = Fixture::with_stored_accounts(vec![
            (Account::from(1), AccountInfo::new_test_instance()),
            (Account::from(2), AccountInfo::new_test_instance()),
        ]);
        let txn = fixture.begin_read_txn();

        let count = fixture.store.count(&txn);

        assert_eq!(count, 2);
    }

    #[test]
    fn delete_account() {
        let fixture = Fixture::new();
        let mut txn = fixture.begin_write_txn();
        let delete_tracker = txn.as_inner_mut().track_deletions();

        let account = Account::from(1);
        fixture.store.del(&mut txn, &account);

        assert_eq!(
            delete_tracker.output(),
            vec![DeleteEvent {
                database: ACCOUNT_TEST_DATABASE.into(),
                key: account.as_bytes().to_vec()
            }]
        )
    }

    #[test]
    fn begin_empty_store_nullable() {
        let fixture = Fixture::new();
        let txn = fixture.begin_read_txn();
        let mut it = fixture.store.iter(&txn);
        assert_eq!(it.next(), None);
    }

    #[test]
    fn begin() {
        let account1 = Account::from(1);
        let account2 = Account::from(2);
        let info1 = AccountInfo {
            head: BlockHash::from(1),
            ..Default::default()
        };
        let info2 = AccountInfo {
            head: BlockHash::from(2),
            ..Default::default()
        };

        let fixture = Fixture::with_stored_accounts(vec![
            (account1.clone(), info1.clone()),
            (account2.clone(), info2.clone()),
        ]);
        let txn = fixture.begin_read_txn();

        let mut it = fixture.store.iter(&txn);
        assert_eq!(it.next(), Some((account1, info1)));
        assert_eq!(it.next(), Some((account2, info2)));
        assert_eq!(it.next(), None);
    }

    #[test]
    fn begin_account() {
        let account1 = Account::from(1);
        let account3 = Account::from(3);
        let info1 = AccountInfo {
            head: BlockHash::from(1),
            ..Default::default()
        };
        let info3 = AccountInfo {
            head: BlockHash::from(3),
            ..Default::default()
        };

        let fixture = Fixture::with_stored_accounts(vec![
            (account1.clone(), info1.clone()),
            (account3.clone(), info3.clone()),
        ]);
        let txn = fixture.begin_read_txn();

        let mut it = fixture.store.iter_range(&txn, Account::from(2)..);

        assert_eq!(it.next(), Some((account3, info3)));
        assert_eq!(it.next(), None);
    }

    #[test]
    fn for_each_par() {
        let account1 = Account::from(1);
        let account3 = Account::from(3);
        let info1 = AccountInfo {
            balance: Amount::raw(1),
            ..Default::default()
        };
        let info3 = AccountInfo {
            balance: Amount::raw(3),
            ..Default::default()
        };

        let fixture = Fixture::with_stored_accounts(vec![
            (account1.clone(), info1.clone()),
            (account3.clone(), info3.clone()),
        ]);

        let balance_sum = Mutex::new(Amount::ZERO);
        fixture.store.for_each_par(&fixture.env, 3, |iter| {
            for (_, info) in iter {
                *balance_sum.lock().unwrap() += info.balance;
            }
        });
        assert_eq!(*balance_sum.lock().unwrap(), Amount::raw(4));
    }

    #[test]
    fn track_inserted_account_info() {
        let fixture = Fixture::new();
        let put_tracker = fixture.store.track_puts();
        let mut txn = fixture.begin_write_txn();
        let account = Account::from(1);
        let info = AccountInfo::new_test_instance();

        fixture.store.put(&mut txn, &account, &info);

        assert_eq!(put_tracker.output(), vec![(account, info)]);
    }
}
