use std::sync::Arc;

use crate::services::TelemetryServices;
use crate::telemetry::{TelementryExt, Telemetry};
use rsnano_messages::TelemetryData;
use rsnano_network::TcpListener;

use super::lifecycle::Lifecycle;

/// Construction-only bundle of telemetry collaborators used to wire up the
/// `TelemetrySubsystem`. This is purely for composition; do not store it on
/// long-lived structs.
#[derive(Clone)]
pub struct TelemetryWiring {
    pub telemetry: Arc<Telemetry>,
    pub tcp_listener: Arc<TcpListener>,
}

#[derive(Clone)]
pub struct TelemetrySubsystem {
    telemetry: Arc<Telemetry>,
    tcp_listener: Arc<TcpListener>,
}

#[derive(Clone)]
pub struct TelemetryTestHandles {
    pub telemetry: Arc<Telemetry>,
    pub tcp_listener: Arc<TcpListener>,
}

impl TelemetrySubsystem {
    pub fn new(telemetry: Arc<Telemetry>, tcp_listener: Arc<TcpListener>) -> Self {
        Self {
            telemetry,
            tcp_listener,
        }
    }

    pub fn local_snapshot(&self) -> TelemetryData {
        self.telemetry.local_telemetry()
    }

    pub fn telemetry(&self) -> Arc<Telemetry> {
        self.telemetry.clone()
    }

    pub fn tcp_listener(&self) -> Arc<TcpListener> {
        self.tcp_listener.clone()
    }

    pub fn telemetry_services(&self) -> TelemetryServices {
        TelemetryServices::new(self.telemetry.clone(), self.tcp_listener.clone())
    }

    /// **Legacy test access - technical debt.**
    ///
    /// This method exposes internal subsystem components for testing.
    /// It is marked hidden and should be avoided in new tests.
    /// Phase 5 will introduce behavioral test helpers to replace this pattern.
    #[doc(hidden)]
    pub fn test_handles(&self) -> TelemetryTestHandles {
        TelemetryTestHandles {
            telemetry: self.telemetry.clone(),
            tcp_listener: self.tcp_listener.clone(),
        }
    }
}

impl Lifecycle for TelemetrySubsystem {
    fn start(&mut self) {
        self.telemetry.start();
    }

    fn stop(&mut self) {
        self.telemetry.stop();
    }
}
