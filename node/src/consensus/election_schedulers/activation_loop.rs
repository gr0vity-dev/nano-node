use std::{
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
    time::Duration,
};

use super::{HintedScheduler, ManualScheduler, OptimisticScheduler, PriorityScheduler};

pub(super) struct SchedulerWakeSignal {
    state: Mutex<SchedulerWakeState>,
    condition: Condvar,
}

impl SchedulerWakeSignal {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(SchedulerWakeState::default()),
            condition: Condvar::new(),
        }
    }

    pub(super) fn wake(&self) {
        {
            let mut guard = self.state.lock().unwrap();
            guard.notified = true;
        }
        self.condition.notify_all();
    }

    pub(super) fn stop(&self) {
        {
            let mut guard = self.state.lock().unwrap();
            guard.stopped = true;
            guard.notified = true;
        }
        self.condition.notify_all();
    }

    fn reset_for_start(&self) {
        let mut guard = self.state.lock().unwrap();
        *guard = SchedulerWakeState::default();
    }

    fn wait(&self, timeout: Duration) -> bool {
        let mut guard = self.state.lock().unwrap();
        if guard.stopped {
            return false;
        }

        if guard.notified {
            guard.notified = false;
            return true;
        }

        let (mut guard_after_wait, _) = self
            .condition
            .wait_timeout_while(guard, timeout, |state| !state.stopped && !state.notified)
            .unwrap();

        if guard_after_wait.stopped {
            return false;
        }

        let was_notified = guard_after_wait.notified;
        guard_after_wait.notified = false;
        was_notified
    }
}

pub(crate) struct ActivationLoop {
    thread: Mutex<Option<JoinHandle<()>>>,
    wake_signal: Arc<SchedulerWakeSignal>,
    priority: Arc<PriorityScheduler>,
    optimistic: Arc<OptimisticScheduler>,
    hinted: Arc<HintedScheduler>,
    manual: Arc<ManualScheduler>,
    enable_priority: bool,
    enable_optimistic: bool,
    enable_hinted: bool,
}

impl ActivationLoop {
    pub(crate) fn new(
        wake_signal: Arc<SchedulerWakeSignal>,
        priority: Arc<PriorityScheduler>,
        optimistic: Arc<OptimisticScheduler>,
        hinted: Arc<HintedScheduler>,
        manual: Arc<ManualScheduler>,
        enable_priority: bool,
        enable_optimistic: bool,
        enable_hinted: bool,
    ) -> Self {
        Self {
            thread: Mutex::new(None),
            wake_signal,
            priority,
            optimistic,
            hinted,
            manual,
            enable_priority,
            enable_optimistic,
            enable_hinted,
        }
    }

    pub(crate) fn start(self: &Arc<Self>) {
        debug_assert!(self.thread.lock().unwrap().is_none());
        self.wake_signal.reset_for_start();

        let activation_loop = Arc::clone(self);
        *self.thread.lock().unwrap() = Some(
            std::thread::Builder::new()
                .name("Sched Act".to_string())
                .spawn(move || activation_loop.run())
                .unwrap(),
        );
    }

    pub(crate) fn stop(&self) {
        self.wake_signal.stop();

        if let Some(handle) = self.thread.lock().unwrap().take() {
            handle.join().unwrap();
        }
    }

    fn run(&self) {
        loop {
            let made_progress = self.run_ready_sources();
            if made_progress {
                continue;
            }

            let timeout = self.wait_timeout();
            if !self.wake_signal.wait(timeout) {
                break;
            }
        }
    }

    fn run_ready_sources(&self) -> bool {
        let mut made_progress = false;

        loop {
            let mut progressed = false;

            if self.manual.run_one() {
                progressed = true;
            }

            if self.enable_priority && self.priority.run_one() {
                progressed = true;
            }

            if self.enable_optimistic && self.optimistic.run_one() {
                progressed = true;
            }

            if self.enable_hinted && self.hinted.run_one() {
                progressed = true;
            }

            if !progressed {
                return made_progress;
            }

            made_progress = true;
        }
    }

    fn wait_timeout(&self) -> Duration {
        let mut timeout = Duration::from_secs(24 * 60 * 60);

        if self.enable_optimistic {
            timeout = timeout.min(self.optimistic.activation_delay());
        }

        if self.enable_hinted {
            timeout = timeout.min(self.hinted.check_interval());
        }

        timeout
    }
}

#[derive(Default)]
struct SchedulerWakeState {
    stopped: bool,
    notified: bool,
}
