use std::sync::{Arc, Condvar, Mutex};

use std::time::Duration;
use store_traits::ledger::{WriteQueueStats, WriteStrategy, WriterType};

#[derive(Clone)]
pub struct WriteQueue {
    inner: Arc<QueueInner>,
}

struct QueueInner {
    state: Mutex<State>,
    cv: Condvar,
}

#[derive(Default)]
struct State {
    pessimistic_active: bool,
    optimistic_active: usize,
    waiting_pessimistic: usize,
    waiting_optimistic: usize,
}

fn stats_from_state(state: &State) -> WriteQueueStats {
    WriteQueueStats {
        pessimistic_active: state.pessimistic_active,
        optimistic_active: state.optimistic_active,
        waiting_pessimistic: state.waiting_pessimistic,
        waiting_optimistic: state.waiting_optimistic,
    }
}

pub struct WriteGuard {
    queue: WriteQueue,
    strategy: WriteStrategy,
    writer_type: WriterType,
}

impl WriteQueue {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(QueueInner {
                state: Mutex::new(State::default()),
                cv: Condvar::new(),
            }),
        }
    }

    pub fn request_write(&self, writer_type: WriterType, strategy: WriteStrategy) -> WriteGuard {
        match strategy {
            WriteStrategy::Optimistic => self.acquire_optimistic(writer_type),
            WriteStrategy::Pessimistic => self.acquire_pessimistic(writer_type),
        }
    }

    pub fn stats(&self) -> WriteQueueStats {
        let state = self.inner.state.lock().unwrap();
        stats_from_state(&state)
    }

    pub fn wait_for<F>(&self, timeout: Duration, predicate: F) -> bool
    where
        F: Fn(&WriteQueueStats) -> bool,
    {
        let guard = self.inner.state.lock().unwrap();
        let (guard, wait_res) = self
            .inner
            .cv
            .wait_timeout_while(guard, timeout, |state| !predicate(&stats_from_state(state)))
            .unwrap();
        predicate(&stats_from_state(&guard)) && !wait_res.timed_out()
    }

    fn acquire_optimistic(&self, writer_type: WriterType) -> WriteGuard {
        let mut guard = self.inner.state.lock().unwrap();
        guard.waiting_optimistic += 1;
        // Wake any observers waiting on queue depth changes.
        self.inner.cv.notify_all();
        while guard.pessimistic_active || guard.waiting_pessimistic > 0 {
            guard = self.inner.cv.wait(guard).unwrap();
        }
        guard.waiting_optimistic -= 1;
        guard.optimistic_active += 1;
        drop(guard);

        WriteGuard {
            queue: self.clone(),
            strategy: WriteStrategy::Optimistic,
            writer_type,
        }
    }

    fn acquire_pessimistic(&self, writer_type: WriterType) -> WriteGuard {
        let mut guard = self.inner.state.lock().unwrap();
        guard.waiting_pessimistic += 1;
        self.inner.cv.notify_all();
        while guard.pessimistic_active || guard.optimistic_active > 0 {
            guard = self.inner.cv.wait(guard).unwrap();
        }
        guard.waiting_pessimistic -= 1;
        guard.pessimistic_active = true;
        drop(guard);

        WriteGuard {
            queue: self.clone(),
            strategy: WriteStrategy::Pessimistic,
            writer_type,
        }
    }
}

impl Drop for WriteGuard {
    fn drop(&mut self) {
        let mut state = self.queue.inner.state.lock().unwrap();
        match self.strategy {
            WriteStrategy::Optimistic => {
                state.optimistic_active = state.optimistic_active.saturating_sub(1);
            }
            WriteStrategy::Pessimistic => {
                state.pessimistic_active = false;
            }
        }
        drop(state);
        self.queue.inner.cv.notify_all();
    }
}

impl WriteGuard {
    pub fn writer_type(&self) -> WriterType {
        self.writer_type
    }

    pub fn strategy(&self) -> WriteStrategy {
        self.strategy
    }
}
