//! BootstrapSubsystem coordinates bootstrap server/responder wiring and runtime lifecycle. Production APIs start/stop and surface state snapshots; raw handles stay test-only.
use std::sync::Arc;

use crate::{
    bootstrap::{BootstrapExt, BootstrapServer, Bootstrapper},
    work::WorkFactory,
};

use super::lifecycle::Lifecycle;
use rsnano_types::Peer;
#[cfg(not(any(test, feature = "test_support")))]
use rsnano_types::Root;
#[cfg(any(test, feature = "test_support"))]
use rsnano_types::{Root, WorkNonce, WorkRequest};

/// Construction-only bundle of bootstrap collaborators used to wire up the
/// `BootstrapSubsystem`. This is purely for composition; do not store it on
/// long-lived structs.
#[derive(Clone)]
pub struct BootstrapWiring {
    bootstrapper: Arc<Bootstrapper>,
    bootstrap_server: Arc<BootstrapServer>,
    work_factory: Arc<WorkFactory>,
}

impl BootstrapWiring {
    pub(crate) fn new(
        bootstrapper: Arc<Bootstrapper>,
        bootstrap_server: Arc<BootstrapServer>,
        work_factory: Arc<WorkFactory>,
    ) -> Self {
        Self {
            bootstrapper,
            bootstrap_server,
            work_factory,
        }
    }
}

#[derive(Clone)]
pub struct BootstrapSubsystem {
    bootstrapper: Arc<Bootstrapper>,
    bootstrap_server: Arc<BootstrapServer>,
    work_factory: Arc<WorkFactory>,
    enable_responder: bool,
}

#[cfg(any(test, feature = "test_support"))]
#[derive(Clone)]
pub struct BootstrapTestHandles {
    bootstrapper: Arc<Bootstrapper>,
    bootstrap_server: Arc<BootstrapServer>,
    work_factory: Arc<WorkFactory>,
}

#[cfg(any(test, feature = "test_support"))]
impl BootstrapTestHandles {
    #[cfg(any(test, feature = "test_support"))] #[doc(hidden)]
    pub fn bootstrapper(&self) -> Arc<Bootstrapper> {
        self.bootstrapper.clone()
    }

    #[cfg(any(test, feature = "test_support"))] #[doc(hidden)]
    pub fn bootstrap_server(&self) -> Arc<BootstrapServer> {
        self.bootstrap_server.clone()
    }

    #[cfg(any(test, feature = "test_support"))] #[doc(hidden)]
    pub fn work_factory(&self) -> Arc<WorkFactory> {
        self.work_factory.clone()
    }
}

impl BootstrapSubsystem {
    pub fn new(wiring: BootstrapWiring, enable_responder: bool) -> Self {
        let BootstrapWiring {
            bootstrapper,
            bootstrap_server,
            work_factory,
        } = wiring;
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

    pub fn work_generation_enabled(&self) -> bool {
        self.work_factory.work_generation_enabled()
    }

    #[cfg(any(test, feature = "test_support"))]
    #[doc(hidden)]
    pub fn generate_work(&self, request: WorkRequest) -> Option<WorkNonce> {
        self.work_factory.generate_work(request)
    }

    pub fn cancel_work(&self, root: Root) {
        self.work_factory.cancel(root);
    }

    pub fn work_peers(&self) -> Vec<Peer> {
        self.work_factory.peers()
    }

    pub fn add_work_peer(&self, peer: Peer) {
        self.work_factory.add_peer(peer);
    }

    pub fn clear_work_peers(&self) {
        self.work_factory.clear_peers();
    }

    /// **Legacy test access - technical debt.**
    ///
    /// This method exposes internal subsystem components for testing.
    /// It is marked hidden and should be avoided in new tests.
    /// Phase 5 will introduce behavioral test helpers to replace this pattern.
    #[cfg(any(test, feature = "test_support"))]
    #[doc(hidden)]
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
