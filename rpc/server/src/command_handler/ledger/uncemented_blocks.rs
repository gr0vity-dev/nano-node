use rsnano_ledger::Ledger;
use rsnano_rpc_messages::{
    UncementedAccountStatus, UncementedBlocksArgs, UncementedBlocksResponse,
};
use rsnano_types::{AccountInfo, ConfirmationHeightInfo};
use store_traits::{LedgerReadTxn, ledger::BlockStore};

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn uncemented_blocks(&self, args: UncementedBlocksArgs) -> UncementedBlocksResponse {
        let ledger = &self.ledger_services.ledger;
        build_uncemented_response(ledger, args)
    }
}

fn build_uncemented_response(
    ledger: &Ledger,
    args: UncementedBlocksArgs,
) -> UncementedBlocksResponse {
    let max_accounts = args.max_accounts.unwrap_or(16);
    let max_blocks_per_account = args.max_blocks_per_account.unwrap_or(32);

    let store = ledger.store_ref();
    let tx = store.begin_read();
    let account_store = store.account();
    let block_store = store.block();
    let conf_store = store.confirmation_height();

    let store_count = block_store.count(tx.as_ref()).unwrap_or(0);
    let cache_count = ledger.block_count();
    let confirmed_count = ledger.confirmed_count();

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
        total_uncemented: total_uncemented.to_string(),
        accounts,
    }
}

fn collect_recent_hashes(
    block_store: &dyn store_traits::ledger::BlockStore,
    tx: &dyn store_traits::LedgerReadTxn,
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
