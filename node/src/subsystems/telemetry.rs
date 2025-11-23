//! TelemetrySubsystem manages telemetry collection and dissemination. Production APIs provide lifecycle and callback hooks; raw telemetry internals remain test-only.
use std::{net::SocketAddrV6, sync::Arc, time::Duration};

use crate::services::TelemetryServices;
use crate::telemetry::{TelementryExt, Telemetry, TelemetrySnapshot};
use rsnano_messages::TelemetryData;
use rsnano_network::TcpListener;

use super::lifecycle::Lifecycle;

/// Construction-only bundle of telemetry collaborators used to wire up the
/// `TelemetrySubsystem`. This is purely for composition; do not store it on
/// long-lived structs.
#[derive(Clone)]
pub(crate) struct TelemetryWiring {
    telemetry: Arc<Telemetry>,
    tcp_listener: Arc<TcpListener>,
}

impl TelemetryWiring {
    pub(crate) fn new(telemetry: Arc<Telemetry>, tcp_listener: Arc<TcpListener>) -> Self {
        Self {
            telemetry,
            tcp_listener,
        }
    }
}

#[derive(Clone)]
pub struct TelemetrySubsystem {
    telemetry: Arc<Telemetry>,
    tcp_listener: Arc<TcpListener>,
}

impl TelemetrySubsystem {
    pub fn new(wiring: TelemetryWiring) -> Self {
        let TelemetryWiring {
            telemetry,
            tcp_listener,
        } = wiring;
        Self {
            telemetry,
            tcp_listener,
        }
    }

    pub fn local_telemetry(&self) -> TelemetryData {
        self.telemetry.local_telemetry()
    }

    pub fn telemetry_for(&self, endpoint: SocketAddrV6) -> Option<TelemetryData> {
        self.telemetry.get_telemetry(&endpoint)
    }

    pub fn all_telemetry(&self) -> Vec<TelemetrySnapshot> {
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
}

impl Lifecycle for TelemetrySubsystem {
    fn start(&mut self) {
        self.telemetry.start();
    }

    fn stop(&mut self) {
        self.telemetry.stop();
    }
}
