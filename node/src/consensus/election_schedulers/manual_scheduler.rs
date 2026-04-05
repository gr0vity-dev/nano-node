use std::{
    collections::VecDeque,
    mem::size_of,
    sync::{Arc, Mutex},
};

use rsnano_ledger::{AnySet, Ledger};
use rsnano_nullable_clock::SteadyClock;
use rsnano_types::{Amount, Block, BlockHash, SavedBlock};
use rsnano_utils::{
    container_info::ContainerInfo,
    stats::{DetailType, StatType, Stats},
};

use crate::consensus::{AecInsertRequest, AecService, election::ElectionBehavior};

pub struct ManualScheduler {
    mutex: Mutex<ManualSchedulerImpl>,
    stats: Arc<Stats>,
    aec: Arc<AecService>,
    clock: Arc<SteadyClock>,
    ledger: Arc<Ledger>,
    wakeup: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl ManualScheduler {
    pub fn new(
        stats: Arc<Stats>,
        active_elections: Arc<AecService>,
        clock: Arc<SteadyClock>,
        ledger: Arc<Ledger>,
    ) -> Self {
        Self {
            stats,
            aec: active_elections,
            clock,
            ledger,
            mutex: Mutex::new(ManualSchedulerImpl {
                queue: Default::default(),
            }),
            wakeup: Mutex::new(None),
        }
    }

    pub fn set_wakeup(&self, wakeup: Arc<dyn Fn() + Send + Sync>) {
        *self.wakeup.lock().unwrap() = Some(wakeup);
    }

    pub fn contains(&self, hash: &BlockHash) -> bool {
        self.mutex
            .lock()
            .unwrap()
            .queue
            .iter()
            .any(|block| block.hash() == *hash)
    }

    pub fn push(&self, block: SavedBlock) {
        {
            let mut guard = self.mutex.lock().unwrap();
            guard.queue.push_back(block);
        }
        if let Some(wakeup) = self.wakeup.lock().unwrap().as_ref() {
            wakeup();
        }
    }

    pub fn run_one(&self) -> bool {
        let block = {
            let mut guard = self.mutex.lock().unwrap();
            let Some(block) = guard.queue.pop_front() else {
                return false;
            };
            block
        };

        self.stats
            .inc(StatType::ElectionScheduler, DetailType::Loop);

        let hash = block.hash();
        let priority = self.ledger.any().block_priority(&block);
        self.stats
            .inc(StatType::ElectionScheduler, DetailType::InsertManual);

        let now = self.clock.now();

        if self
            .aec
            .insert(AecInsertRequest::new_manual(block, priority), now)
            .is_ok()
        {
            self.aec.transition_active(&hash);
        }

        true
    }

    pub fn container_info(&self) -> ContainerInfo {
        let guard = self.mutex.lock().unwrap();
        [(
            "queue",
            guard.queue.len(),
            size_of::<Arc<Block>>() + size_of::<Option<Amount>>() + size_of::<ElectionBehavior>(),
        )]
        .into()
    }
}

struct ManualSchedulerImpl {
    queue: VecDeque<SavedBlock>,
}
