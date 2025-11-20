use super::lifecycle::Lifecycle;

/// Facade over generic ticker pool scheduling.
pub struct TickerSubsystem {
    _private: (),
}

impl TickerSubsystem {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Schedule a named periodic task.
    pub fn schedule(&self, _label: &str) {
        todo!("schedule task")
    }
}

impl Lifecycle for TickerSubsystem {
    fn start(&mut self) {
        todo!("start ticker pool")
    }

    fn stop(&mut self) {
        todo!("stop ticker pool")
    }
}
