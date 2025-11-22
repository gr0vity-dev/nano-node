use std::{collections::HashMap, net::SocketAddrV6, sync::Arc, time::Duration};

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
    pub fn new(wiring: TelemetryWiring) -> Self {
        let TelemetryWiring {
            telemetry,
            tcp_listener,
        } = wiring;
        Self { telemetry, tcp_listener }
    }

    pub fn local_snapshot(&self) -> TelemetryData {
        self.telemetry.local_telemetry()
    }

    pub fn local_telemetry(&self) -> TelemetryData {
        self.telemetry.local_telemetry()
    }

    pub fn telemetry_for(&self, endpoint: &SocketAddrV6) -> Option<TelemetryData> {
        self.telemetry.get_telemetry(endpoint)
    }

    pub fn all_telemetries(&self) -> HashMap<SocketAddrV6, TelemetryData> {
        self.telemetry.get_all_telemetries()
    }

    pub fn listener_address(&self) -> SocketAddrV6 {
        self.tcp_listener.local_address()
    }

    pub fn uptime(&self) -> Duration {
        self.telemetry.startup_time.elapsed()
    }

    pub fn on_telemetry_processed(
        &self,
        callback: Box<dyn Fn(&TelemetryData, &SocketAddrV6) + Send + Sync>,
    ) {
        self.telemetry.on_telemetry_processed(callback);
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
