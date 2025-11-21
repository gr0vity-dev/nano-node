use crate::command_handler::RpcCommandHandler;
use anyhow::bail;
use rsnano_node::{Node, handles::LedgerQueryHandle};
use rsnano_rpc_messages::{ExistsResponse, ReceivableExistsArgs};
use rsnano_types::{PendingKey, SavedBlock};
use std::sync::Arc;

impl RpcCommandHandler {
    pub(crate) fn receivable_exists(
        &self,
        args: ReceivableExistsArgs,
    ) -> anyhow::Result<ExistsResponse> {
        let include_active = args.include_active.unwrap_or_default().inner();
        let include_only_confirmed = args.include_only_confirmed.unwrap_or(true.into()).inner();
        let ledger = self.ledger_queries.clone();

        let Some(block) = ledger.get_block(&args.hash) else {
            bail!(Self::BLOCK_NOT_FOUND);
        };

        let mut exists = if block.is_send() {
            let pending_key = PendingKey::new(block.destination().unwrap(), args.hash);
            ledger.get_pending(&pending_key).is_some()
        } else {
            false
        };

        if exists {
            exists = block_confirmed(
                self.node.clone(),
                &ledger,
                &block,
                include_active,
                include_only_confirmed,
            );
        }
        Ok(ExistsResponse::new(exists))
    }
}

/** Due to the asynchronous nature of updating confirmation heights, it can also be necessary to check active roots */
fn block_confirmed(
    node: Arc<Node>,
    ledger: &LedgerQueryHandle,
    block: &SavedBlock,
    include_active: bool,
    include_only_confirmed: bool,
) -> bool {
    if include_active && !include_only_confirmed {
        return true;
    }

    // Check whether the confirmation height is set
    if ledger.confirmed_block_exists(&block.hash()) {
        return true;
    }

    // This just checks it's not currently undergoing an active transaction
    if !include_only_confirmed {
        return !node.is_active_root(&block.qualified_root());
    }

    false
}
