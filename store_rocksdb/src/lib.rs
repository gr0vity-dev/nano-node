pub mod account_store;
pub mod block_store;
pub mod confirmation_height_store;
pub mod final_vote_store;
pub mod online_weight_store;
pub mod peer_store;
pub mod pending_store;
pub mod pruned_store;
pub mod rep_weight_store;
pub mod successor_store;
pub mod version_store;

mod environment;
mod ledger_impl;
mod ledger_store_factory;
mod transaction;
mod utils;

pub use account_store::RocksdbAccountStore;
pub use block_store::RocksdbBlockStore;
pub use confirmation_height_store::RocksdbConfirmationHeightStore;
pub use environment::{RocksdbStoreEnvironment, RocksdbStoreEnvironmentFactory};
pub use final_vote_store::RocksdbFinalVoteStore;
pub use ledger_impl::{RocksdbLedgerStore, rocksdb_vendor};
pub use ledger_store_factory::RocksdbLedgerStoreFactory;
pub use online_weight_store::RocksdbOnlineWeightStore;
pub use peer_store::RocksdbPeerStore;
pub use pending_store::RocksdbPendingStore;
pub use pruned_store::RocksdbPrunedStore;
pub use rep_weight_store::RocksdbRepWeightStore;
pub use successor_store::RocksdbSuccessorStore;
pub use transaction::{RocksdbLedgerReadTxn, RocksdbLedgerWriteTxn};
pub use version_store::RocksdbVersionStore;

pub(crate) use environment::{
    ACCOUNTS_CF_NAME, BLOCK_DATA_CF_NAME, BLOCK_INDEX_CF_NAME, CONF_HEIGHT_CF_NAME,
    FINAL_VOTE_CF_NAME, ONLINE_WEIGHT_CF_NAME, PEERS_CF_NAME, PENDING_CF_NAME, PRUNED_CF_NAME,
    REP_WEIGHT_CF_NAME, SUCCESSOR_CF_NAME, VERSION_CF_NAME,
};
pub(crate) use transaction::{RocksdbCursor, rocksdb_ro_cursor_from_store};
pub(crate) use utils::value_in_range;

#[cfg(test)]
#[path = "../build_support.rs"]
mod build_support;

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_types::{
        Account, AccountInfo, Amount, Block, BlockHash, ConfirmationHeightInfo, PendingInfo,
        PendingKey, PrivateKey, PublicKey, QualifiedRoot, SavedBlock,
    };
    use std::{
        fs,
        net::{Ipv6Addr, SocketAddrV6},
        ops::Bound,
        sync::Arc,
        time::{Duration, UNIX_EPOCH},
    };
    use store_traits::{
        config::RocksDbConfig,
        environment::{StoreCursor, StoreEnvironment, StoreReadTxn, StoreWriteTxn},
        ledger::RangeBounds,
        transaction::LedgerWriteTxn,
        types::{StoreEnvironmentFlags, StoreWriteFlags},
    };
    use tempfile::tempdir;

    #[test]
    fn vendor_matches_librocksdb_version() {
        let vendor = rocksdb_vendor();
        assert_eq!(vendor.name, "rocksdb");

        let lock_path = build_support::workspace_lock_path().expect("workspace Cargo.lock");
        let contents = fs::read_to_string(lock_path).expect("read Cargo.lock");
        let version = build_support::find_version(&contents, "rust-librocksdb-sys")
            .or_else(|| build_support::find_version(&contents, "librocksdb-sys"))
            .expect("librocksdb-sys version");
        let expected = version
            .split('+')
            .nth(1)
            .unwrap_or(version.as_str())
            .to_string();

        assert_eq!(vendor.version, expected);
    }

    struct BlockFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbBlockStore,
    }

    impl BlockFixture {
        fn new() -> Self {
            let dir = tempdir().unwrap();
            let env = Arc::new(
                RocksdbStoreEnvironment::open(
                    dir.path().to_path_buf(),
                    StoreEnvironmentFlags::empty(),
                    Some(dir),
                    Some(&RocksDbConfig::default()),
                )
                .unwrap(),
            );
            let store = RocksdbBlockStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }
    }

    fn create_env() -> Arc<RocksdbStoreEnvironment> {
        let dir = tempdir().unwrap();
        let env = RocksdbStoreEnvironment::open(
            dir.path().to_path_buf(),
            StoreEnvironmentFlags::empty(),
            Some(dir),
            Some(&RocksDbConfig::default()),
        )
        .unwrap();
        Arc::new(env)
    }

    struct AccountFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbAccountStore,
    }

    impl AccountFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbAccountStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_accounts(&self, entries: &[(Account, AccountInfo)]) {
            let mut txn = self.begin_write();
            for (account, info) in entries {
                self.store.put(&mut txn, account, info);
            }
            Box::new(txn).commit().expect("rocksdb test commit failed");
        }
    }

    struct PendingFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbPendingStore,
    }

    impl PendingFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbPendingStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_entries(&self, entries: &[(PendingKey, PendingInfo)]) {
            let mut txn = self.begin_write();
            for (key, info) in entries {
                self.store.put(&mut txn, key, info);
            }
            Box::new(txn).commit().expect("rocksdb test commit failed");
        }
    }

    struct ConfirmationFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbConfirmationHeightStore,
    }

    impl ConfirmationFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbConfirmationHeightStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_entries(&self, entries: &[(Account, ConfirmationHeightInfo)]) {
            let mut txn = self.begin_write();
            for (account, info) in entries {
                self.store.put(&mut txn, account, info);
            }
            Box::new(txn).commit().expect("rocksdb test commit failed");
        }
    }

    struct RepWeightFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbRepWeightStore,
    }

    impl RepWeightFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbRepWeightStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_entries(&self, entries: &[(PublicKey, Amount)]) {
            let mut txn = self.begin_write();
            for (account, weight) in entries {
                self.store.put(&mut txn, *account, *weight);
            }
            Box::new(txn).commit().expect("rocksdb test commit failed");
        }
    }

    struct SuccessorFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbSuccessorStore,
    }

    impl SuccessorFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbSuccessorStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_entries(&self, entries: &[(BlockHash, BlockHash)]) {
            let mut txn = self.begin_write();
            for (block, successor) in entries {
                self.store.put(&mut txn, block, successor);
            }
            Box::new(txn).commit().expect("rocksdb test commit failed");
        }
    }

    struct OnlineWeightFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbOnlineWeightStore,
    }

    impl OnlineWeightFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbOnlineWeightStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }

        fn insert_entries(&self, entries: &[(u64, Amount)]) {
            let mut txn = self.begin_write();
            for (time, amount) in entries {
                self.store.put(&mut txn, *time, amount);
            }
            Box::new(txn).commit().expect("rocksdb test commit failed");
        }
    }

    struct PrunedFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbPrunedStore,
    }

    impl PrunedFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbPrunedStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }
    }

    struct FinalVoteFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbFinalVoteStore,
    }

    impl FinalVoteFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbFinalVoteStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }
    }

    struct PeerFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbPeerStore,
    }

    impl PeerFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbPeerStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }
    }

    struct VersionFixture {
        env: Arc<RocksdbStoreEnvironment>,
        store: RocksdbVersionStore,
    }

    impl VersionFixture {
        fn new() -> Self {
            let env = create_env();
            let store = RocksdbVersionStore::new(Arc::clone(&env)).unwrap();
            Self { env, store }
        }

        fn begin_read(&self) -> RocksdbLedgerReadTxn {
            RocksdbLedgerReadTxn::new(&self.env)
        }

        fn begin_write(&self) -> RocksdbLedgerWriteTxn {
            RocksdbLedgerWriteTxn::new(&self.env)
        }
    }

    #[test]
    fn write_and_read_roundtrip() {
        let env = create_env();
        let database = env.open_db(Some("blocks")).unwrap();

        {
            let mut txn = env.begin_write();
            txn.put(database, b"key", b"value", StoreWriteFlags::empty())
                .unwrap();
            txn.commit().expect("rocksdb write txn commit failed");
        }

        let txn = env.begin_read();
        let value = txn.get(database, b"key").unwrap();
        assert_eq!(value.as_ref(), b"value");
    }

    #[test]
    fn write_txn_drop_discards_changes() {
        let env = create_env();
        let database = env.open_db(Some("accounts")).unwrap();

        {
            let mut txn = env.begin_write();
            txn.put(database, b"key", b"value", StoreWriteFlags::empty())
                .unwrap();
            // Transaction dropped without commit
        }

        let read_txn = env.begin_read();
        let err = read_txn.get(database, b"key").unwrap_err();
        assert!(err.is_not_found());
    }

    #[test]
    fn write_txn_explicit_rollback() {
        let env = create_env();
        let database = env.open_db(Some("accounts")).unwrap();
        let mut txn = env.begin_write();
        txn.put(database, b"rollback", b"value", StoreWriteFlags::empty())
            .unwrap();
        drop(txn);

        let read_txn = env.begin_read();
        assert!(read_txn.get(database, b"rollback").is_err());
    }

    #[test]
    fn write_txn_reads_own_writes() {
        let env = create_env();
        let database = env.open_db(None).unwrap();

        let mut txn = env.begin_write();
        txn.put(database, b"pending", b"123", StoreWriteFlags::empty())
            .unwrap();
        let value = txn.get(database, b"pending").unwrap();
        assert_eq!(value.as_ref(), b"123");
    }

    #[test]
    fn read_txn_snapshot_isolation() {
        let env = create_env();
        let database = env.open_db(Some("pending")).unwrap();

        {
            let mut txn = env.begin_write();
            txn.put(database, b"snapshot", b"v1", StoreWriteFlags::empty())
                .unwrap();
            txn.commit().expect("rocksdb write txn commit failed");
        }

        let read_txn = env.begin_read();
        let initial = read_txn.get(database, b"snapshot").unwrap();
        assert_eq!(initial.as_ref(), b"v1");

        {
            let mut write_txn = env.begin_write();
            write_txn
                .put(database, b"snapshot", b"v2", StoreWriteFlags::empty())
                .unwrap();
            write_txn.commit().expect("rocksdb write txn commit failed");
        }

        // Existing read transaction should continue to see the original value.
        let snapshot_value = read_txn.get(database, b"snapshot").unwrap();
        assert_eq!(snapshot_value.as_ref(), b"v1");

        // A fresh read transaction gets the updated value.
        let fresh_read = env.begin_read();
        let updated = fresh_read.get(database, b"snapshot").unwrap();
        assert_eq!(updated.as_ref(), b"v2");
    }

    #[test]
    fn cursor_reflects_overlay() {
        let env = create_env();
        let database = env.open_db(Some("accounts")).unwrap();

        let mut txn = env.begin_write();
        txn.put(database, b"a", b"1", StoreWriteFlags::empty())
            .unwrap();
        txn.put(database, b"b", b"2", StoreWriteFlags::empty())
            .unwrap();
        let mut cursor = txn.open_rw_cursor(database).unwrap();

        let first = cursor.next().unwrap().unwrap();
        assert_eq!(first.0.as_ref(), b"a");
        assert_eq!(first.1.as_ref(), b"1");
        let second = cursor.next().unwrap().unwrap();
        assert_eq!(second.0.as_ref(), b"b");
        assert_eq!(second.1.as_ref(), b"2");
    }

    #[test]
    fn write_txn_cursor_streams_snapshot_and_overlay() {
        let env = create_env();
        let database = env.open_db(Some("stream_merge")).unwrap();

        {
            let mut init = env.begin_write();
            init.put(database, b"b", b"base_b", StoreWriteFlags::empty())
                .unwrap();
            init.put(database, b"d", b"base_d", StoreWriteFlags::empty())
                .unwrap();
            init.commit().expect("rocksdb test commit failed");
        }

        let mut txn = env.begin_write();
        txn.put(database, b"a", b"overlay_a", StoreWriteFlags::empty())
            .unwrap();
        txn.delete(database, b"b", None).unwrap();
        txn.put(database, b"c", b"overlay_c", StoreWriteFlags::empty())
            .unwrap();

        let mut cursor = txn.open_rw_cursor(database).unwrap();
        let mut entries = Vec::new();
        while let Some((key, value)) = cursor.next().unwrap() {
            entries.push((key.to_vec(), value.to_vec()));
        }

        assert_eq!(
            entries,
            vec![
                (b"a".to_vec(), b"overlay_a".to_vec()),
                (b"c".to_vec(), b"overlay_c".to_vec()),
                (b"d".to_vec(), b"base_d".to_vec()),
            ]
        );
    }

    #[test]
    fn write_txn_count_streams_overlay() {
        let env = create_env();
        let database = env.open_db(Some("stream_count")).unwrap();

        {
            let mut init = env.begin_write();
            init.put(database, b"a", b"1", StoreWriteFlags::empty())
                .unwrap();
            init.put(database, b"b", b"2", StoreWriteFlags::empty())
                .unwrap();
            init.commit().expect("rocksdb test commit failed");
        }

        let mut txn = env.begin_write();
        txn.delete(database, b"a", None).unwrap();
        txn.put(database, b"c", b"3", StoreWriteFlags::empty())
            .unwrap();

        assert_eq!(txn.count(database).unwrap(), 2);
    }

    #[test]
    fn write_txn_clear_streams_overlay() {
        let env = create_env();
        let database = env.open_db(Some("stream_clear")).unwrap();

        {
            let mut init = env.begin_write();
            init.put(database, b"x", b"1", StoreWriteFlags::empty())
                .unwrap();
            init.put(database, b"y", b"2", StoreWriteFlags::empty())
                .unwrap();
            init.commit().expect("rocksdb test commit failed");
        }

        let mut txn = env.begin_write();
        txn.clear_db(database).unwrap();
        txn.put(database, b"z", b"3", StoreWriteFlags::empty())
            .unwrap();

        assert!(txn.get(database, b"x").is_err());
        assert_eq!(txn.count(database).unwrap(), 1);

        let mut cursor = txn.open_rw_cursor(database).unwrap();
        let first = cursor.next().unwrap().unwrap();
        assert_eq!(first.0.as_ref(), b"z");
        assert_eq!(first.1.as_ref(), b"3");
        assert!(cursor.next().unwrap().is_none());
    }

    #[test]
    fn write_txn_reads_put_before_commit() {
        let env = create_env();
        let database = env.open_db(Some("read_own_put")).unwrap();

        let mut txn = env.begin_write();
        txn.put(database, b"alpha", b"value", StoreWriteFlags::empty())
            .unwrap();
        let fetched = txn.get(database, b"alpha").unwrap();
        assert_eq!(fetched.as_ref(), b"value");
        Box::new(txn)
            .commit()
            .expect("rocksdb write txn commit failed");
    }

    #[test]
    fn write_txn_reads_delete_before_commit() {
        let env = create_env();
        let database = env.open_db(Some("read_own_delete")).unwrap();

        {
            let mut init = env.begin_write();
            init.put(database, b"beta", b"persisted", StoreWriteFlags::empty())
                .unwrap();
            init.commit().expect("rocksdb write txn commit failed");
        }

        let mut txn = env.begin_write();
        txn.delete(database, b"beta", None).unwrap();
        let result = txn.get(database, b"beta");
        assert!(result.is_err() && result.unwrap_err().is_not_found());
        Box::new(txn)
            .commit()
            .expect("rocksdb write txn commit failed");
    }

    #[test]
    fn write_txn_clear_then_put_only_sees_new_entry() {
        let env = create_env();
        let database = env.open_db(Some("clear_then_put")).unwrap();

        {
            let mut init = env.begin_write();
            init.put(database, b"old", b"value", StoreWriteFlags::empty())
                .unwrap();
            init.commit().expect("rocksdb write txn commit failed");
        }

        let mut txn = env.begin_write();
        txn.clear_db(database).unwrap();
        txn.put(database, b"new", b"value", StoreWriteFlags::empty())
            .unwrap();

        let fetched = txn.get(database, b"new").unwrap();
        assert_eq!(fetched.as_ref(), b"value");
        assert!(txn.get(database, b"old").is_err());

        let mut cursor = txn.open_rw_cursor(database).unwrap();
        let first = cursor.next().unwrap().unwrap();
        assert_eq!(first.0.as_ref(), b"new");
        assert!(cursor.next().unwrap().is_none());
    }

    #[test]
    fn write_txn_cursor_orders_mixed_changes() {
        let env = create_env();
        let database = env.open_db(Some("cursor_mixed")).unwrap();

        {
            let mut init = env.begin_write();
            init.put(database, b"b", b"base_b", StoreWriteFlags::empty())
                .unwrap();
            init.put(database, b"d", b"base_d", StoreWriteFlags::empty())
                .unwrap();
            init.commit().expect("rocksdb write txn commit failed");
        }

        let mut txn = env.begin_write();
        txn.put(database, b"a", b"overlay_a", StoreWriteFlags::empty())
            .unwrap();
        txn.delete(database, b"b", None).unwrap();
        txn.put(database, b"c", b"overlay_c", StoreWriteFlags::empty())
            .unwrap();
        txn.put(database, b"e", b"overlay_e", StoreWriteFlags::empty())
            .unwrap();

        let mut cursor = txn.open_rw_cursor(database).unwrap();
        let mut entries = Vec::new();
        while let Some((key, value)) = cursor.next().unwrap() {
            entries.push((key.to_vec(), value.to_vec()));
        }

        assert_eq!(
            entries,
            vec![
                (b"a".to_vec(), b"overlay_a".to_vec()),
                (b"c".to_vec(), b"overlay_c".to_vec()),
                (b"d".to_vec(), b"base_d".to_vec()),
                (b"e".to_vec(), b"overlay_e".to_vec())
            ]
        );
    }

    #[test]
    fn block_store_put_get() {
        let fixture = BlockFixture::new();
        let block = SavedBlock::new_test_open_block();
        let mut write_txn = fixture.begin_write();
        fixture.store.put(&mut write_txn, &block);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &block.hash()), Some(block));
    }

    #[test]
    fn block_store_delete() {
        let fixture = BlockFixture::new();
        let block = SavedBlock::new_test_open_block();
        let mut write_txn = fixture.begin_write();
        fixture.store.put(&mut write_txn, &block);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let mut delete_txn = fixture.begin_write();
        fixture.store.del(&mut delete_txn, &block.hash());
        Box::new(delete_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &block.hash()).is_none());
    }

    #[test]
    fn block_store_iterates() {
        let fixture = BlockFixture::new();
        let mut write_txn = fixture.begin_write();
        for seed in 0..3 {
            let block = unique_block(seed);
            fixture.store.put(&mut write_txn, &block);
        }
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        let count = fixture.store.iter(&read_txn).count();
        assert_eq!(count, 3);
    }

    #[test]
    fn account_store_put_get() {
        let fixture = AccountFixture::new();
        let tracker = fixture.store.track_puts();
        let account = Account::from(42);
        let info = AccountInfo::new_test_instance();

        let mut write_txn = fixture.begin_write();
        fixture.store.put(&mut write_txn, &account, &info);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &account), Some(info.clone()));
        assert_eq!(tracker.output(), vec![(account, info)]);
    }

    #[test]
    fn account_store_delete() {
        let fixture = AccountFixture::new();
        let entries = vec![
            (Account::from(1), AccountInfo::new_test_instance()),
            (Account::from(2), AccountInfo::new_test_instance()),
        ];
        fixture.insert_accounts(&entries);

        let mut write_txn = fixture.begin_write();
        fixture.store.del(&mut write_txn, &entries[0].0);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &entries[0].0).is_none());
        assert!(fixture.store.get(&read_txn, &entries[1].0).is_some());
    }

    #[test]
    fn account_store_iterates_in_order() {
        let fixture = AccountFixture::new();
        let entries = vec![
            (Account::from(1), AccountInfo::new_test_instance()),
            (Account::from(3), AccountInfo::new_test_instance()),
            (Account::from(2), AccountInfo::new_test_instance()),
        ];
        fixture.insert_accounts(&entries);

        let read_txn = fixture.begin_read();
        let accounts: Vec<_> = fixture
            .store
            .iter(&read_txn)
            .map(|(account, _)| account)
            .collect();
        assert_eq!(
            accounts,
            vec![Account::from(1), Account::from(2), Account::from(3)]
        );
    }

    #[test]
    fn account_store_iter_range() {
        let fixture = AccountFixture::new();
        let entries = vec![
            (Account::from(10), AccountInfo::new_test_instance()),
            (Account::from(20), AccountInfo::new_test_instance()),
            (Account::from(30), AccountInfo::new_test_instance()),
        ];
        fixture.insert_accounts(&entries);

        let read_txn = fixture.begin_read();
        let range = RangeBounds::new(
            Bound::Included(Account::from(15)),
            Bound::Excluded(Account::from(30)),
        );
        let accounts: Vec<_> = fixture
            .store
            .iter_range(&read_txn, range)
            .map(|(account, _)| account)
            .collect();
        assert_eq!(accounts, vec![Account::from(20)]);
    }

    #[test]
    fn account_store_count() {
        let fixture = AccountFixture::new();
        let entries = vec![
            (Account::from(1), AccountInfo::new_test_instance()),
            (Account::from(2), AccountInfo::new_test_instance()),
        ];
        fixture.insert_accounts(&entries);

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 2);
    }

    #[test]
    fn pending_store_not_found() {
        let fixture = PendingFixture::new();
        let read_txn = fixture.begin_read();
        let key = PendingKey::new_test_instance();
        assert!(fixture.store.get(&read_txn, &key).is_none());
        assert!(!fixture.store.exists(&read_txn, &key));
    }

    #[test]
    fn pending_store_put_get() {
        let fixture = PendingFixture::new();
        let key = PendingKey::new_test_instance();
        let info = PendingInfo::new_test_instance();
        let mut write_txn = fixture.begin_write();
        let tracker = fixture.store.track_puts();
        fixture.store.put(&mut write_txn, &key, &info);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &key), Some(info.clone()));
        assert_eq!(tracker.output(), vec![(key, info)]);
    }

    #[test]
    fn pending_store_delete() {
        let fixture = PendingFixture::new();
        let key = PendingKey::new_test_instance();
        let info = PendingInfo::new_test_instance();
        fixture.insert_entries(&[(key.clone(), info)]);

        let mut write_txn = fixture.begin_write();
        let tracker = fixture.store.track_deletions();
        fixture.store.del(&mut write_txn, &key);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");
        assert_eq!(tracker.output(), vec![key.clone()]);

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &key).is_none());
    }

    #[test]
    fn pending_store_iter_empty() {
        let fixture = PendingFixture::new();
        let read_txn = fixture.begin_read();
        assert!(fixture.store.iter(&read_txn).next().is_none());
    }

    #[test]
    fn pending_store_iterates() {
        let fixture = PendingFixture::new();
        let key = PendingKey::new_test_instance();
        let info = PendingInfo::new_test_instance();
        fixture.insert_entries(&[(key.clone(), info.clone())]);

        let read_txn = fixture.begin_read();
        let entries: Vec<_> = fixture.store.iter(&read_txn).collect();
        assert_eq!(entries, vec![(key, info)]);
    }

    #[test]
    fn pending_store_iter_range() {
        let fixture = PendingFixture::new();
        let k1 = PendingKey::new(Account::from(1), BlockHash::from(1));
        let k2 = PendingKey::new(Account::from(2), BlockHash::from(1));
        let k3 = PendingKey::new(Account::from(3), BlockHash::from(1));
        let info = PendingInfo::new_test_instance();
        fixture.insert_entries(&[(k1, info.clone()), (k2, info.clone()), (k3, info.clone())]);

        let read_txn = fixture.begin_read();
        let range = RangeBounds::new(
            Bound::Included(PendingKey::new(Account::from(2), BlockHash::from(0))),
            Bound::Excluded(PendingKey::new(Account::from(3), BlockHash::from(0))),
        );
        let entries: Vec<_> = fixture.store.iter_range(&read_txn, range).collect();
        assert_eq!(entries, vec![(k2, info)]);
    }

    #[test]
    fn pending_store_exists() {
        let fixture = PendingFixture::new();
        let key = PendingKey::new_test_instance();
        let info = PendingInfo::new_test_instance();
        fixture.insert_entries(&[(key.clone(), info)]);
        let read_txn = fixture.begin_read();
        assert!(fixture.store.exists(&read_txn, &key));
    }

    #[test]
    fn pending_store_any_for_account() {
        let fixture = PendingFixture::new();
        let account = Account::from(42);
        let key = PendingKey::new(account, BlockHash::from(7));
        let info = PendingInfo::new_test_instance();
        fixture.insert_entries(&[(key, info)]);

        let read_txn = fixture.begin_read();
        assert!(fixture.store.any(&read_txn, &account));
        assert!(!fixture.store.any(&read_txn, &Account::from(5)));
    }

    #[test]
    fn confirmation_store_empty() {
        let fixture = ConfirmationFixture::new();
        let read_txn = fixture.begin_read();
        let account = Account::from(1);
        assert!(fixture.store.get(&read_txn, &account).is_none());
        assert!(!fixture.store.exists(&read_txn, &account));
        assert!(fixture.store.iter(&read_txn).next().is_none());
    }

    #[test]
    fn confirmation_store_put_get() {
        let fixture = ConfirmationFixture::new();
        let account = Account::from(2);
        let info = ConfirmationHeightInfo::new(5, BlockHash::from(9));
        let mut txn = fixture.begin_write();
        fixture.store.put(&mut txn, &account, &info);
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &account), Some(info.clone()));
        assert!(fixture.store.exists(&read_txn, &account));
        assert_eq!(fixture.store.count(&read_txn), 1);
    }

    #[test]
    fn confirmation_store_delete() {
        let fixture = ConfirmationFixture::new();
        let account = Account::from(3);
        let info = ConfirmationHeightInfo::new(2, BlockHash::from(5));
        fixture.insert_entries(&[(account, info)]);

        let mut txn = fixture.begin_write();
        fixture.store.del(&mut txn, &Account::from(3));
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &Account::from(3)).is_none());
    }

    #[test]
    fn confirmation_store_iter_range() {
        let fixture = ConfirmationFixture::new();
        let entries = vec![
            (
                Account::from(1),
                ConfirmationHeightInfo::new(1, BlockHash::from(1)),
            ),
            (
                Account::from(2),
                ConfirmationHeightInfo::new(2, BlockHash::from(2)),
            ),
            (
                Account::from(3),
                ConfirmationHeightInfo::new(3, BlockHash::from(3)),
            ),
        ];
        fixture.insert_entries(&entries);

        let read_txn = fixture.begin_read();
        let range = RangeBounds::new(
            Bound::Included(Account::from(2)),
            Bound::Excluded(Account::from(3)),
        );
        let entries: Vec<_> = fixture.store.iter_range(&read_txn, range).collect();
        assert_eq!(
            entries,
            vec![(
                Account::from(2),
                ConfirmationHeightInfo::new(2, BlockHash::from(2))
            )]
        );
    }

    #[test]
    fn confirmation_store_clear() {
        let fixture = ConfirmationFixture::new();
        let entries = vec![(
            Account::from(1),
            ConfirmationHeightInfo::new(1, BlockHash::from(1)),
        )];
        fixture.insert_entries(&entries);

        let mut txn = fixture.begin_write();
        fixture.store.clear(&mut txn);
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 0);
    }

    #[test]
    fn rep_weight_count() {
        let fixture = RepWeightFixture::new();
        let entries = vec![
            (PublicKey::from(1), Amount::from(10)),
            (PublicKey::from(2), Amount::from(20)),
        ];
        fixture.insert_entries(&entries);
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 2);
    }

    #[test]
    fn rep_weight_put_get() {
        let fixture = RepWeightFixture::new();
        let mut write_txn = fixture.begin_write();
        let put_tracker = fixture.store.track_puts();
        let account = PublicKey::from(5);
        let weight = Amount::from(50);
        fixture.store.put(&mut write_txn, account, weight);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &account), Some(weight));
        assert_eq!(put_tracker.output(), vec![(account, weight)]);
    }

    #[test]
    fn rep_weight_delete() {
        let fixture = RepWeightFixture::new();
        let account = PublicKey::from(7);
        fixture.insert_entries(&[(account, Amount::from(70))]);

        let mut write_txn = fixture.begin_write();
        let delete_tracker = fixture.store.track_deletions();
        fixture.store.del(&mut write_txn, &account);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &account).is_none());
        assert_eq!(delete_tracker.output(), vec![account]);
    }

    #[test]
    fn rep_weight_iter_empty() {
        let fixture = RepWeightFixture::new();
        let read_txn = fixture.begin_read();
        assert!(fixture.store.iter(&read_txn).next().is_none());
    }

    #[test]
    fn rep_weight_iterates() {
        let fixture = RepWeightFixture::new();
        let entries = vec![
            (PublicKey::from(1), Amount::from(100)),
            (PublicKey::from(2), Amount::from(200)),
        ];
        fixture.insert_entries(&entries);

        let read_txn = fixture.begin_read();
        let items: Vec<_> = fixture.store.iter(&read_txn).collect();
        assert_eq!(items, entries);
    }

    #[test]
    fn successor_store_count() {
        let fixture = SuccessorFixture::new();
        let entries = vec![
            (BlockHash::from(1), BlockHash::from(2)),
            (BlockHash::from(3), BlockHash::from(4)),
        ];
        fixture.insert_entries(&entries);
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 2);
    }

    #[test]
    fn successor_store_put_get() {
        let fixture = SuccessorFixture::new();
        let mut txn = fixture.begin_write();
        let tracker = fixture.store.track_puts();
        let block = BlockHash::from(10);
        let successor = BlockHash::from(11);
        fixture.store.put(&mut txn, &block, &successor);
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &block), Some(successor));
        assert_eq!(tracker.output(), vec![(block, successor)]);
    }

    #[test]
    fn successor_store_delete() {
        let fixture = SuccessorFixture::new();
        let block = BlockHash::from(5);
        let successor = BlockHash::from(6);
        fixture.insert_entries(&[(block, successor)]);

        let mut txn = fixture.begin_write();
        fixture.store.del(&mut txn, &block);
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &block).is_none());
    }

    #[test]
    fn successor_store_no_entry() {
        let fixture = SuccessorFixture::new();
        let read_txn = fixture.begin_read();
        assert!(
            fixture
                .store
                .get(&read_txn, &BlockHash::from(999))
                .is_none()
        );
    }

    #[test]
    fn online_weight_empty() {
        let fixture = OnlineWeightFixture::new();
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 0);
        assert!(fixture.store.iter(&read_txn).next().is_none());
        assert!(fixture.store.iter_rev(&read_txn).next().is_none());
    }

    #[test]
    fn online_weight_put_get() {
        let fixture = OnlineWeightFixture::new();
        let mut txn = fixture.begin_write();
        fixture.store.put(&mut txn, 1, &Amount::from(100));
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        let entries: Vec<_> = fixture.store.iter(&read_txn).collect();
        assert_eq!(entries, vec![(1, Amount::from(100))]);
    }

    #[test]
    fn online_weight_iter_rev() {
        let fixture = OnlineWeightFixture::new();
        fixture.insert_entries(&[(1, Amount::from(10)), (2, Amount::from(20))]);
        let read_txn = fixture.begin_read();
        let entries: Vec<_> = fixture.store.iter_rev(&read_txn).collect();
        assert_eq!(entries, vec![(2, Amount::from(20)), (1, Amount::from(10))]);
    }

    #[test]
    fn online_weight_delete() {
        let fixture = OnlineWeightFixture::new();
        fixture.insert_entries(&[(5, Amount::from(50))]);
        let mut txn = fixture.begin_write();
        fixture.store.del(&mut txn, 5);
        Box::new(txn).commit().expect("rocksdb test commit failed");
        let read_txn = fixture.begin_read();
        assert!(fixture.store.iter(&read_txn).next().is_none());
    }

    #[test]
    fn online_weight_clear() {
        let fixture = OnlineWeightFixture::new();
        fixture.insert_entries(&[(7, Amount::from(70))]);
        let mut txn = fixture.begin_write();
        fixture.store.clear(&mut txn);
        Box::new(txn).commit().expect("rocksdb test commit failed");
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 0);
    }

    #[test]
    fn pruned_store_put_exists() {
        let fixture = PrunedFixture::new();
        let mut txn = fixture.begin_write();
        let hash = BlockHash::from(100);
        fixture.store.put(&mut txn, &hash);
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert!(fixture.store.exists(&read_txn, &hash));
    }

    #[test]
    fn pruned_store_delete() {
        let fixture = PrunedFixture::new();
        let mut txn = fixture.begin_write();
        let hash = BlockHash::from(200);
        fixture.store.put(&mut txn, &hash);
        fixture.store.del(&mut txn, &hash);
        Box::new(txn).commit().expect("rocksdb test commit failed");
        let read_txn = fixture.begin_read();
        assert!(!fixture.store.exists(&read_txn, &hash));
    }

    #[test]
    fn pruned_store_count() {
        let fixture = PrunedFixture::new();
        let mut txn = fixture.begin_write();
        fixture.store.put(&mut txn, &BlockHash::from(1));
        fixture.store.put(&mut txn, &BlockHash::from(2));
        Box::new(txn).commit().expect("rocksdb test commit failed");
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 2);
    }

    #[test]
    fn final_vote_put_and_get() {
        let fixture = FinalVoteFixture::new();
        let root = QualifiedRoot::new_test_instance();
        let hash = BlockHash::from(123);
        let mut txn = fixture.begin_write();
        assert!(fixture.store.put(&mut txn, &root, &hash));
        Box::new(txn).commit().expect("rocksdb test commit failed");
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn, &root), Some(hash));
    }

    #[test]
    fn final_vote_conflict_detection() {
        let fixture = FinalVoteFixture::new();
        let root = QualifiedRoot::new_test_instance();
        let mut txn = fixture.begin_write();
        assert!(fixture.store.put(&mut txn, &root, &BlockHash::from(1)));
        assert!(!fixture.store.put(&mut txn, &root, &BlockHash::from(2)));
    }

    #[test]
    fn final_vote_delete_and_clear() {
        let fixture = FinalVoteFixture::new();
        let root = QualifiedRoot::new_test_instance();
        let hash = BlockHash::from(42);

        let mut insert_txn = fixture.begin_write();
        fixture.store.put(&mut insert_txn, &root, &hash);
        Box::new(insert_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let mut delete_txn = fixture.begin_write();
        fixture.store.del(&mut delete_txn, &root);
        Box::new(delete_txn)
            .commit()
            .expect("rocksdb test commit failed");
        let read_txn = fixture.begin_read();
        assert!(fixture.store.get(&read_txn, &root).is_none());

        let mut reinsertion_txn = fixture.begin_write();
        fixture.store.put(&mut reinsertion_txn, &root, &hash);
        Box::new(reinsertion_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let mut clear_txn = fixture.begin_write();
        fixture.store.clear(&mut clear_txn);
        Box::new(clear_txn)
            .commit()
            .expect("rocksdb test commit failed");
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 0);
    }

    #[test]
    fn peer_store_put_tracks() {
        let fixture = PeerFixture::new();
        let mut txn = fixture.begin_write();
        let tracker = fixture.store.track_puts();
        let addr = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 7000, 0, 0);
        let time = UNIX_EPOCH + Duration::from_secs(10);

        fixture.store.put(&mut txn, addr, time);
        assert_eq!(tracker.output(), vec![(addr, time)]);
    }

    #[test]
    fn peer_store_delete_tracks() {
        let fixture = PeerFixture::new();
        let mut txn = fixture.begin_write();
        let tracker = fixture.store.track_deletions();
        let addr = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 7001, 0, 0);

        fixture.store.del(&mut txn, addr);
        assert_eq!(tracker.output(), vec![addr]);
    }

    #[test]
    fn peer_store_exists_and_iterates() {
        let fixture = PeerFixture::new();
        let mut txn = fixture.begin_write();
        let addr1 = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 7100, 0, 0);
        let addr2 = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 7101, 0, 0);
        fixture
            .store
            .put(&mut txn, addr1, UNIX_EPOCH + Duration::from_secs(1));
        fixture
            .store
            .put(&mut txn, addr2, UNIX_EPOCH + Duration::from_secs(2));
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert!(fixture.store.exists(&read_txn, addr1));
        let peers: Vec<_> = fixture.store.iter(&read_txn).collect();
        assert_eq!(peers.len(), 2);
    }

    #[test]
    fn peer_store_clear() {
        let fixture = PeerFixture::new();
        let mut txn = fixture.begin_write();
        let addr = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 7200, 0, 0);
        fixture
            .store
            .put(&mut txn, addr, UNIX_EPOCH + Duration::from_secs(3));
        Box::new(txn).commit().expect("rocksdb test commit failed");

        let mut clear_txn = fixture.begin_write();
        fixture.store.clear(&mut clear_txn);
        Box::new(clear_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&read_txn), 0);
    }

    #[test]
    fn version_store_initially_empty() {
        let fixture = VersionFixture::new();
        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn), None);
    }

    #[test]
    fn version_store_put_and_get() {
        let fixture = VersionFixture::new();
        let mut write_txn = fixture.begin_write();
        fixture.store.put(&mut write_txn, 42);
        Box::new(write_txn)
            .commit()
            .expect("rocksdb test commit failed");

        let read_txn = fixture.begin_read();
        assert_eq!(fixture.store.get(&read_txn), Some(42));
    }

    fn unique_block(seed: u8) -> SavedBlock {
        let key = PrivateKey::from(u64::from(seed) + 42);
        let block = Block::new_test_instance_with_key(key);
        SavedBlock::new_test_instance_with(block)
    }
}
