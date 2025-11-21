use std::time::SystemTime;

use rsnano_ledger::{BlockStore, DuplicateInsertRecordSnapshot, Ledger, LedgerReadTxn};
use rsnano_node::block_processing::BlockSource;
use rsnano_rpc_messages::{
    DuplicateInsertEntry, UncementedAccountStatus, UncementedBlocksArgs, UncementedBlocksResponse,
    UncementedInsertSource,
};
use rsnano_types::{AccountInfo, ConfirmationHeightInfo};
use strum::IntoEnumIterator;

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn uncemented_blocks(&self, args: UncementedBlocksArgs) -> UncementedBlocksResponse {
        let ledger = &self.ledger_services.ledger_arc();
        build_uncemented_response(ledger, args)
    }
}

fn build_uncemented_response(
    ledger: &Ledger,
    args: UncementedBlocksArgs,
) -> UncementedBlocksResponse {
    let max_accounts = args.max_accounts.unwrap_or(16);
    let max_blocks_per_account = args.max_blocks_per_account.unwrap_or(32);

    let store = ledger.store.as_ref();
    let tx = store.begin_read();
    let account_store = store.account();
    let block_store = store.block();
    let conf_store = store.confirmation_height();

    let store_count = block_store.iter(tx.as_ref()).count() as u64;
    let cache_count = ledger.block_count();
    let confirmed_count = ledger.confirmed_count();
    let cache_inserts = ledger.block_cache_inserts();
    let cache_rollbacks = ledger.block_cache_rollbacks();
    let insert_sources_snapshot = ledger.block_cache_insert_sources();
    let duplicate_inserts = ledger.block_cache_duplicate_inserts();
    let duplicate_sources_snapshot = ledger.block_cache_duplicate_sources();
    let duplicate_events_snapshot = ledger.duplicate_insert_log();

    let mut accounts = Vec::new();
    let mut total_uncemented = 0u64;

    for (account, info) in account_store.iter(tx.as_ref()) {
        let conf_info = conf_store
            .get(tx.as_ref(), &account)
            .unwrap_or_else(|| ConfirmationHeightInfo::new(0, Default::default()));
        if info.block_count > conf_info.height {
            let missing = info.block_count - conf_info.height;
            total_uncemented += missing;

            let sample_hashes = collect_recent_hashes(
                block_store,
                tx.as_ref(),
                &info,
                conf_info.height,
                max_blocks_per_account,
            );

            accounts.push(UncementedAccountStatus {
                account: account.encode_account(),
                head: info.head.to_string(),
                confirmed_frontier: conf_info.frontier.to_string(),
                missing_count: missing.to_string(),
                sample_hashes,
            });

            if accounts.len() >= max_accounts {
                break;
            }
        }
    }

    UncementedBlocksResponse {
        cache_count: cache_count.to_string(),
        store_count: store_count.to_string(),
        confirmed_count: confirmed_count.to_string(),
        cache_inserts: cache_inserts.to_string(),
        cache_rollbacks: cache_rollbacks.to_string(),
        duplicate_inserts: duplicate_inserts.to_string(),
        total_uncemented: total_uncemented.to_string(),
        accounts,
        insert_sources: build_insert_sources(insert_sources_snapshot),
        duplicate_sources: build_insert_sources(duplicate_sources_snapshot),
        duplicate_events: build_duplicate_events(duplicate_events_snapshot),
    }
}

fn build_insert_sources(snapshot: Vec<u64>) -> Vec<UncementedInsertSource> {
    BlockSource::iter()
        .map(|source| {
            let idx = source as usize;
            let count = snapshot.get(idx).copied().unwrap_or_default();
            UncementedInsertSource {
                source: source.as_str().to_string(),
                inserts: count.to_string(),
            }
        })
        .collect()
}

fn build_duplicate_events(
    snapshot: Vec<DuplicateInsertRecordSnapshot>,
) -> Vec<DuplicateInsertEntry> {
    snapshot
        .into_iter()
        .map(|record| {
            let first = BlockSource::from_u8(record.first_source);
            let second = BlockSource::from_u8(record.second_source);
            let timestamp = record
                .timestamp
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            DuplicateInsertEntry {
                hash: record.hash.to_string(),
                first_source: first.as_str().to_string(),
                second_source: second.as_str().to_string(),
                timestamp,
            }
        })
        .collect()
}

fn collect_recent_hashes(
    block_store: &dyn BlockStore,
    tx: &dyn LedgerReadTxn,
    info: &AccountInfo,
    confirmed_height: u64,
    max_hashes: usize,
) -> Vec<String> {
    let mut hashes = Vec::new();
    let mut current_hash = info.head;
    let mut current_height = info.block_count;

    while current_height > confirmed_height && hashes.len() < max_hashes {
        hashes.push(current_hash.to_string());
        if current_hash.is_zero() {
            break;
        }
        match block_store.get(tx, &current_hash) {
            Some(block) => {
                current_hash = block.previous();
                if current_height == 0 {
                    break;
                }
                current_height -= 1;
            }
            None => break,
        }
    }

    hashes
}
