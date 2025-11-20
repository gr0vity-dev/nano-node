use super::lifecycle::Lifecycle;

/// Facade over telemetry collection/cache.
pub struct TelemetrySubsystem {
    _private: (),
}

impl TelemetrySubsystem {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Return the local telemetry snapshot.
    pub fn local_snapshot(&self) -> String {
        todo!("return local telemetry")
    }

    /// Request telemetry from all peers.
    pub fn request_all(&self) {
        todo!("request telemetry from peers")
    }
}

impl Lifecycle for TelemetrySubsystem {
    fn start(&mut self) {
        todo!("start telemetry polling")
    }

    fn stop(&mut self) {
        todo!("stop telemetry polling")
    }
}
