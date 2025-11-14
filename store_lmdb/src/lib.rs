#[macro_use]
extern crate anyhow;

mod account_store;
mod block_store;
mod confirmation_height_store;
mod environment;
mod fan;
mod final_vote_store;
#[cfg(feature = "ledger_snapshots")]
pub mod forks_store;
mod iterator;
mod ledger_impl;
mod ledger_store_factory;
mod lmdb_config;
mod online_weight_store;
mod peer_store;
mod pending_store;
mod rep_weight_store;
mod store;
mod store_utils;
mod successor_store;
mod transaction;
mod upgrades;
mod vacuum;
mod version_store;
mod wallet_env;
mod wallet_factory;
mod wallet_store;
mod wallet_txn_shim;

use primitive_types::U256;

use rsnano_nullable_lmdb::LmdbDatabase;

pub use account_store::{ConfiguredAccountDatabaseBuilder, LmdbAccountStore};
pub use block_store::{ConfiguredBlockDatabaseBuilder, LmdbBlockStore};
pub use confirmation_height_store::*;
pub use environment::{LmdbStoreEnvironment, LmdbStoreEnvironmentFactory};
pub use fan::Fan;
pub use final_vote_store::LmdbFinalVoteStore;
pub use iterator::{LmdbIterator, LmdbRangeIterator};
pub use ledger_store_factory::{LmdbLedgerStoreFactory, create_null_store_with_databases};
pub use lmdb_config::{LmdbConfig, SyncStrategy, default_ledger_lmdb_options, get_lmdb_flags};
pub use online_weight_store::LmdbOnlineWeightStore;
pub use peer_store::*;
pub use pending_store::{ConfiguredPendingDatabaseBuilder, LmdbPendingStore, read_pending_record};
pub use rep_weight_store::*;
pub use rsnano_nullable_lmdb::EnvironmentFlags;
pub use rsnano_nullable_lmdb::EnvironmentOptions;
pub use store::LmdbStore;
pub use store_traits::ledger::{LedgerCache, MemoryStats};
pub use store_traits::wallet::{KeyType, WalletValue};
pub use successor_store::LmdbSuccessorStore;
pub use transaction::{LmdbLedgerReadTxn, LmdbLedgerWriteTxn};
pub use upgrades::create_and_update_lmdb_env;
pub use vacuum::vacuum;
pub use version_store::LmdbVersionStore;
pub use wallet_env::{LmdbWalletEnvironment, LmdbWalletEnvironmentFactory};
pub use wallet_factory::LmdbWalletStoreFactory;
pub use wallet_store::{Fans, LmdbWalletStore};
pub use wallet_txn_shim::{WalletReadTxnSHIM, WalletWriteTxnSHIM};

struct Split {
    start: U256,
    end: U256,
    is_last: bool,
}

pub(crate) fn parallel_traversal(
    thread_count: usize,
    action: &(impl Fn(U256, U256, bool) + Send + Sync),
) {
    debug_assert!(thread_count > 0);
    let split = U256::max_value() / thread_count;

    let splits: Vec<_> = (0..thread_count)
        .map(|i| Split {
            start: split * i,
            end: split * (i + 1),
            is_last: i == thread_count - 1,
        })
        .collect();

    std::thread::scope(|s| {
        for split in &splits[1..] {
            std::thread::Builder::new()
                .name("DB par traversl".to_owned())
                .spawn_scoped(s, move || {
                    action(split.start, split.end, split.is_last);
                })
                .unwrap();
        }

        let first = &splits[0];
        action(first.start, first.end, first.is_last);
    });
}

pub const STORE_VERSION_MINIMUM: i32 = 24;

/// RsNano uses store versions upwards of 10_000 so that there is a clear
/// distinction between databases from nano_node and RsNano
pub const STORE_VERSION_CURRENT: i32 = 10_001;

/// The first store version where RsNano is incompatible with nano_node
pub const FIRST_INCOMPATIBLE_STORE_VERSION: i32 = 10_000;

pub const BLOCK_INDEX_DATABASE: LmdbDatabase = LmdbDatabase::new_null(1);
pub const BLOCK_DATA_DATABASE: LmdbDatabase = LmdbDatabase::new_null(2);
pub const FRONTIER_TEST_DATABASE: LmdbDatabase = LmdbDatabase::new_null(3);
pub const ACCOUNT_TEST_DATABASE: LmdbDatabase = LmdbDatabase::new_null(4);
pub const PENDING_TEST_DATABASE: LmdbDatabase = LmdbDatabase::new_null(5);
pub const PRUNED_TEST_DATABASE: LmdbDatabase = LmdbDatabase::new_null(6);
pub const REP_WEIGHT_TEST_DATABASE: LmdbDatabase = LmdbDatabase::new_null(7);
pub const CONFIRMATION_HEIGHT_TEST_DATABASE: LmdbDatabase = LmdbDatabase::new_null(8);
pub const PEERS_TEST_DATABASE: LmdbDatabase = LmdbDatabase::new_null(9);
pub const FORKS_TEST_DATABASE: LmdbDatabase = LmdbDatabase::new_null(10);

#[cfg(test)]
mod test {
    use super::*;
    use rsnano_nullable_lmdb::{DatabaseFlags, DeleteEvent, LmdbEnvironment};

    #[test]
    fn tracks_deletes() {
        let env = LmdbEnvironment::new_null();
        let database = env.create_db(Some("foo"), DatabaseFlags::empty()).unwrap();

        let mut tx = env.begin_write();
        let delete_tracker = tx.track_deletions();

        let key = vec![1, 2, 3];
        tx.delete(database, &key, None).unwrap();

        assert_eq!(delete_tracker.output(), vec![DeleteEvent { database, key }])
    }

    #[test]
    fn tracks_clears() {
        let env = LmdbEnvironment::new_null();
        let mut txn = env.begin_write();
        let clear_tracker = txn.track_clears();

        let database = LmdbDatabase::new_null(42);
        txn.clear_db(database).unwrap();

        assert_eq!(clear_tracker.output(), vec![database])
    }
}
