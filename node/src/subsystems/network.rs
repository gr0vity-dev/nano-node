use super::lifecycle::Lifecycle;

/// Facade over network internals (listener, peer connector, filters, queues).
/// Concrete wiring will replace the placeholders during encapsulation work.
pub struct NetworkSubsystem {
    _private: (),
}

impl NetworkSubsystem {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Returns the local listening endpoint for peer discovery.
    pub fn local_endpoint(&self) -> String {
        todo!("return tcp listener address")
    }

    /// Current connected peer count.
    pub fn peer_count(&self) -> usize {
        todo!("compute peer count")
    }

    /// Broadcast a keepalive to peers.
    pub fn broadcast_keepalive(&self) {
        todo!("send keepalive")
    }

    /// Flood a serialized publish message to peers.
    pub fn flood_publish(&self, _bytes: &[u8]) {
        todo!("flood publish message")
    }

    /// Enqueue an outbound message with QoS hints.
    pub fn send_message(&self, _label: &str, _payload: &[u8]) {
        todo!("enqueue outbound message")
    }
}

impl Lifecycle for NetworkSubsystem {
    fn start(&mut self) {
        todo!("start network threads, listeners, processors")
    }

    fn stop(&mut self) {
        todo!("stop network threads, listeners, processors")
    }
}
