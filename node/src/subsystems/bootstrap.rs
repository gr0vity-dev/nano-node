use std::sync::Arc;

use crate::{
    bootstrap::{BootstrapExt, BootstrapServer, Bootstrapper},
    work::WorkFactory,
};

use super::lifecycle::Lifecycle;

#[derive(Clone)]
pub struct BootstrapSubsystem {
    bootstrapper: Arc<Bootstrapper>,
    bootstrap_server: Arc<BootstrapServer>,
    work_factory: Arc<WorkFactory>,
    enable_responder: bool,
}

#[derive(Clone)]
pub struct BootstrapTestHandles {
    pub bootstrapper: Arc<Bootstrapper>,
    pub bootstrap_server: Arc<BootstrapServer>,
    pub work_factory: Arc<WorkFactory>,
}

impl BootstrapSubsystem {
    pub fn new(
        bootstrapper: Arc<Bootstrapper>,
        bootstrap_server: Arc<BootstrapServer>,
        work_factory: Arc<WorkFactory>,
        enable_responder: bool,
    ) -> Self {
        Self {
            bootstrapper,
            bootstrap_server,
            work_factory,
            enable_responder,
        }
    }

    pub fn start_bootstrap(&self) {
        self.bootstrapper.start();
    }

    pub fn cancel_bootstrap(&self) {
        self.bootstrapper.stop();
    }

    pub fn start_responder_if_enabled(&self) {
        if self.enable_responder {
            self.bootstrap_server.start();
        }
    }

    pub fn test_handles(&self) -> BootstrapTestHandles {
        BootstrapTestHandles {
            bootstrapper: self.bootstrapper.clone(),
            bootstrap_server: self.bootstrap_server.clone(),
            work_factory: self.work_factory.clone(),
        }
    }
}

impl Lifecycle for BootstrapSubsystem {
    fn start(&mut self) {
        self.start_responder_if_enabled();
        self.start_bootstrap();
    }

    fn stop(&mut self) {
        self.bootstrapper.stop();
        self.bootstrap_server.stop();
        self.work_factory.stop();
    }
}
