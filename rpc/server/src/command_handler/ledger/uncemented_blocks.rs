use std::time::SystemTime;

use rsnano_ledger::DuplicateInsertRecordSnapshot;
use rsnano_node::{
    block_processing::BlockSource,
    handles::{UncementedAccountSnapshot, UncementedBlocksSnapshot},
};
use rsnano_rpc_messages::{
    DuplicateInsertEntry, UncementedAccountStatus, UncementedBlocksArgs, UncementedBlocksResponse,
    UncementedInsertSource,
};
use strum::IntoEnumIterator;

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn uncemented_blocks(&self, args: UncementedBlocksArgs) -> UncementedBlocksResponse {
        let max_accounts = args.max_accounts.unwrap_or(16) as usize;
        let max_blocks_per_account = args.max_blocks_per_account.unwrap_or(32) as usize;
        let snapshot = self
            .ledger_queries
            .uncemented_blocks_snapshot(max_accounts, max_blocks_per_account);
        build_uncemented_response(snapshot)
    }
}

fn build_uncemented_response(snapshot: UncementedBlocksSnapshot) -> UncementedBlocksResponse {
    UncementedBlocksResponse {
        cache_count: snapshot.cache_count.to_string(),
        store_count: snapshot.store_count.to_string(),
        confirmed_count: snapshot.confirmed_count.to_string(),
        cache_inserts: snapshot.cache_inserts.to_string(),
        cache_rollbacks: snapshot.cache_rollbacks.to_string(),
        duplicate_inserts: snapshot.duplicate_inserts.to_string(),
        total_uncemented: snapshot.total_uncemented.to_string(),
        accounts: snapshot
            .accounts
            .into_iter()
            .map(into_account_status)
            .collect(),
        insert_sources: build_insert_sources(snapshot.insert_sources),
        duplicate_sources: build_insert_sources(snapshot.duplicate_sources),
        duplicate_events: build_duplicate_events(snapshot.duplicate_events),
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

fn into_account_status(account: UncementedAccountSnapshot) -> UncementedAccountStatus {
    UncementedAccountStatus {
        account: account.account.encode_account(),
        head: account.head.to_string(),
        confirmed_frontier: account.confirmed_frontier.to_string(),
        missing_count: account.missing_count.to_string(),
        sample_hashes: account
            .sample_hashes
            .into_iter()
            .map(|h| h.to_string())
            .collect(),
    }
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
