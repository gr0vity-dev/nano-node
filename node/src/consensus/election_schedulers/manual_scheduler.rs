use std::{
    collections::VecDeque,
    mem::size_of,
    sync::{Arc, Condvar, Mutex, RwLock},
    thread::JoinHandle,
};

use rsnano_ledger::{AnySet, Ledger};
use rsnano_nullable_clock::SteadyClock;
use rsnano_types::{Amount, Block, BlockHash, SavedBlock};
use rsnano_utils::{
    container_info::ContainerInfo,
    stats::{DetailType, StatType, Stats},
};

use crate::consensus::{
    ActiveElectionsContainer, AecEventPublisher, AecInsertRequest, election::ElectionBehavior,
};

pub struct ManualScheduler {
    thread: Mutex<Option<JoinHandle<()>>>,
    condition: Condvar,
    mutex: Mutex<ManualSchedulerImpl>,
    stats: Arc<Stats>,
    active_elections: Arc<RwLock<ActiveElectionsContainer>>,
    clock: Arc<SteadyClock>,
    ledger: Arc<Ledger>,
    publisher: AecEventPublisher,
}

impl ManualScheduler {
    pub(crate) fn new(
        stats: Arc<Stats>,
        active_elections: Arc<RwLock<ActiveElectionsContainer>>,
        clock: Arc<SteadyClock>,
        ledger: Arc<Ledger>,
        publisher: AecEventPublisher,
    ) -> Self {
        Self {
            thread: Mutex::new(None),
            condition: Condvar::new(),
            stats,
            active_elections,
            clock,
            ledger,
            publisher,
            mutex: Mutex::new(ManualSchedulerImpl {
                queue: Default::default(),
                stopped: false,
            }),
        }
    }

    pub fn stop(&self) {
        self.mutex.lock().unwrap().stopped = true;
        self.notify();
        let handle = self.thread.lock().unwrap().take();
        if let Some(handle) = handle {
            handle.join().unwrap();
        }
    }

    pub fn contains(&self, hash: &BlockHash) -> bool {
        self.mutex
            .lock()
            .unwrap()
            .queue
            .iter()
            .any(|block| block.hash() == *hash)
    }

    pub fn notify(&self) {
        self.condition.notify_all();
    }

    pub fn push(&self, block: SavedBlock) {
        {
            let mut guard = self.mutex.lock().unwrap();
            guard.queue.push_back(block);
        }
        self.notify();
    }

    fn run(&self) {
        let mut guard = self.mutex.lock().unwrap();
        while !guard.stopped {
            guard = self
                .condition
                .wait_while(guard, |g| !g.stopped && !g.predicate())
                .unwrap();

            if !guard.stopped {
                self.stats
                    .inc(StatType::ElectionScheduler, DetailType::Loop);

                if guard.predicate() {
                    let block = guard.queue.pop_front().unwrap();
                    drop(guard);

                    let hash = block.hash();
                    let priority = self.ledger.any().block_priority(&block);
                    self.stats
                        .inc(StatType::ElectionScheduler, DetailType::InsertManual);

                    let now = self.clock.now();

                    let events = {
                        let mut aec = self.active_elections.write().unwrap();
                        match aec.insert(AecInsertRequest::new_manual(block, priority), now) {
                            Ok(result) => {
                                aec.transition_active(&hash);
                                result.events
                            }
                            Err(_) => Vec::new(),
                        }
                    };
                    self.publisher.publish_all(events);
                } else {
                    drop(guard);
                }
                self.notify();
                guard = self.mutex.lock().unwrap();
            }
        }
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

impl Drop for ManualScheduler {
    fn drop(&mut self) {
        // Thread must be stopped before destruction
        debug_assert!(self.thread.lock().unwrap().is_none());
    }
}

pub trait ManualSchedulerExt {
    fn start(&self);
}

impl ManualSchedulerExt for Arc<ManualScheduler> {
    fn start(&self) {
        debug_assert!(self.thread.lock().unwrap().is_none());
        let self_l = Arc::clone(self);
        *self.thread.lock().unwrap() = Some(
            std::thread::Builder::new()
                .name("Sched Manual".to_string())
                .spawn(Box::new(move || {
                    self_l.run();
                }))
                .unwrap(),
        )
    }
}

struct ManualSchedulerImpl {
    queue: VecDeque<SavedBlock>,
    stopped: bool,
}

impl ManualSchedulerImpl {
    fn predicate(&self) -> bool {
        !self.queue.is_empty()
    }
}
