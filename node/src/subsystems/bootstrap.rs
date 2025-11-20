use super::lifecycle::Lifecycle;

/// Facade over bootstrapper/server/work factory components.
pub struct BootstrapSubsystem {
    _private: (),
}

impl BootstrapSubsystem {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Start a legacy/bootstrap responder if enabled.
    pub fn start_responder(&self) {
        todo!("start bootstrap responder")
    }

    /// Kick off a bootstrap attempt.
    pub fn start_bootstrap(&self) {
        todo!("start bootstrap attempt")
    }

    /// Cancel ongoing bootstrap operations.
    pub fn cancel_bootstrap(&self) {
        todo!("cancel bootstrap")
    }
}

impl Lifecycle for BootstrapSubsystem {
    fn start(&mut self) {
        todo!("start bootstrap server/worker")
    }

    fn stop(&mut self) {
        todo!("stop bootstrap server/worker")
    }
}
