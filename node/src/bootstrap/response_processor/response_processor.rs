use std::sync::{Arc, Mutex};
use tracing::trace;

use rsnano_ledger::Ledger;
use rsnano_messages::AscPullAck;
use rsnano_network::ChannelId;
use rsnano_nullable_clock::Timestamp;
use rsnano_utils::stats::Stats;

use super::super::state::BootstrapLogic;
use crate::{
    bootstrap::{
        response_processor::frontier_check_pool::FrontierCheckPool,
        state::bootstrap_logic::{ProcessError, ProcessInfo},
    },
    services::block_submitter::BlockSubmitter,
};

pub(crate) struct ResponseProcessor {
    logic: Arc<Mutex<BootstrapLogic>>,
    frontier_check_pool: FrontierCheckPool,
    block_submitter: Arc<BlockSubmitter>,
}

impl ResponseProcessor {
    pub(crate) fn new(
        logic: Arc<Mutex<BootstrapLogic>>,
        stats: Arc<Stats>,
        block_submitter: Arc<BlockSubmitter>,
        ledger: Arc<Ledger>,
    ) -> Self {
        let frontier_check_pool = FrontierCheckPool::new(stats.clone(), ledger, logic.clone());

        Self {
            logic,
            frontier_check_pool,
            block_submitter,
        }
    }

    pub fn set_max_pending_frontiers(&mut self, max_pending: usize) {
        self.frontier_check_pool.max_pending = max_pending;
    }

    pub fn process(
        &self,
        response: AscPullAck,
        channel_id: ChannelId,
        now: Timestamp,
    ) -> Result<ProcessInfo, ProcessError> {
        trace!(query_id = response.id, ?channel_id, "Process response");

        let mut logic = self.logic.lock().unwrap();
        let process_info = logic.process_response(response, channel_id, now)?;
        self.enqueue_next_blocks(&mut logic);
        self.frontier_check_pool.enqueue_frontiers(&mut logic);
        Ok(process_info)
    }

    // TODO Remeove duplication! Copied from BlockInspector
    fn enqueue_next_blocks(&self, logic: &mut BootstrapLogic) {
        while let Some((block, query_id)) = logic.block_ack_processor.block_queue.next_to_process()
        {
            let block_hash = block.hash();

            trace!(%block_hash, query_id, "Process block");

            match self
                .block_submitter
                // TODO use real channel id
                .submit_bootstrap(block.clone(), ChannelId::LOOPBACK)
            {
                Ok(_) => {
                    logic
                        .block_ack_processor
                        .block_queue
                        .enqueued_for_processing(&block_hash);
                }
                Err(err) => {
                    trace!(%block_hash, query_id, ?err, "failed to enqueue bootstrap block");
                    break;
                }
            }
        }
    }
}
