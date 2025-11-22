use crate::BacklogServices;

use super::lifecycle::Lifecycle;

/// Lifecycle wrapper for backlog scanning services.
pub struct BacklogSubsystem {
    services: BacklogServices,
}

impl BacklogSubsystem {
    pub fn new(services: BacklogServices) -> Self {
        Self { services }
    }

    pub fn services(&self) -> &BacklogServices {
        &self.services
    }
}

impl Lifecycle for BacklogSubsystem {
    fn start(&mut self) {
        self.services.start();
    }

    fn stop(&mut self) {
        self.services.stop();
    }
}
