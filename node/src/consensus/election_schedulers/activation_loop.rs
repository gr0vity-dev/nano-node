use std::{
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
    time::Duration,
};

use super::{HintedScheduler, ManualScheduler, OptimisticScheduler, PriorityScheduler};

pub(crate) struct SchedulerWakeSignal {
    state: Mutex<WakeState>,
    condition: Condvar,
}

impl SchedulerWakeSignal {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(WakeState::default()),
            condition: Condvar::new(),
        }
    }

    pub(crate) fn prepare_to_start(&self) {
        let mut guard = self.state.lock().unwrap();
        guard.stopped = false;
        guard.notified = false;
    }

    pub(crate) fn stop(&self) {
        {
            let mut guard = self.state.lock().unwrap();
            guard.stopped = true;
            guard.notified = true;
        }
        self.condition.notify_all();
    }

    pub(crate) fn wake(&self) {
        {
            let mut guard = self.state.lock().unwrap();
            guard.notified = true;
        }
        self.condition.notify_all();
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.state.lock().unwrap().stopped
    }

    pub(crate) fn take_notification(&self) -> bool {
        let mut guard = self.state.lock().unwrap();
        let notified = guard.notified;
        guard.notified = false;
        notified
    }

    pub(crate) fn wait(&self, timeout: Duration) -> bool {
        let guard = self.state.lock().unwrap();
        let (mut guard_after_wait, _) = self
            .condition
            .wait_timeout_while(guard, timeout, |state| !state.stopped && !state.notified)
            .unwrap();

        if guard_after_wait.stopped {
            return true;
        }

        guard_after_wait.notified = false;
        false
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SchedulerChange {
    SourceQueueGainedWork,
    VacancyReleased,
    SchedulingRelevantVoteObserved,
}

pub(crate) struct SchedulerWakePolicy {
    wake_signal: Arc<SchedulerWakeSignal>,
}

impl SchedulerWakePolicy {
    pub(crate) fn new(wake_signal: Arc<SchedulerWakeSignal>) -> Self {
        Self { wake_signal }
    }

    pub(crate) fn notify(&self, _change: SchedulerChange) {
        self.wake_signal.wake();
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
        self.wake_signal.prepare_to_start();

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
            if self.wake_signal.is_stopped() {
                break;
            }

            if made_progress || self.wake_signal.take_notification() {
                continue;
            }

            if self.wake_signal.wait(self.wait_timeout()) {
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
struct WakeState {
    stopped: bool,
    notified: bool,
}
