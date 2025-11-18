use std::sync::{Arc, Mutex};

use rsnano_ledger::Ledger;
use rsnano_utils::{stats::Stats, thread_pool::ThreadPool};

use crate::bootstrap::{
    response_processor::frontier_worker::FrontierWorker, state::BootstrapLogic,
};

pub(crate) struct FrontierCheckPool {
    stats: Arc<Stats>,
    ledger: Arc<Ledger>,
    logic: Arc<Mutex<BootstrapLogic>>,
    workers: Arc<ThreadPool>,
    pub max_pending: usize,
}

impl FrontierCheckPool {
    pub(crate) fn new(
        stats: Arc<Stats>,
        ledger: Arc<Ledger>,
        state: Arc<Mutex<BootstrapLogic>>,
    ) -> Self {
        let workers = Arc::new(ThreadPool::new(1, "Bootstrap work"));
        Self {
            stats,
            ledger,
            logic: state,
            workers,
            max_pending: 16,
        }
    }

    pub(crate) fn enqueue_frontiers(&self, logic: &mut BootstrapLogic) {
        while let Some(frontiers) = logic.frontiers_processor.pop_frontiers_to_check() {
            let ledger = self.ledger.clone();
            let stats = self.stats.clone();
            let state = self.logic.clone();
            self.workers.execute(move || {
                let mut worker = FrontierWorker::new(ledger, &stats, &state);
                worker.process(frontiers);
            });
        }
        let queued_tasks = self.workers.queued_count();
        logic
            .frontiers_processor
            .set_frontier_checker_overfill(queued_tasks >= self.max_pending);
    }
}
