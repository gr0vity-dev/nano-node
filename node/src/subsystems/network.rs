use std::{
    net::SocketAddrV6,
    sync::{Arc, Mutex, RwLock},
};

use rsnano_messages::NetworkFilter;
use rsnano_network::{Channel, Network, PeerConnector, TcpListener, TcpListenerExt};
use rsnano_network_protocol::InboundMessageQueue;
use rsnano_nullable_clock::SteadyClock;
use tracing::warn;

use crate::transport::keepalive::KeepalivePublisher;
use crate::transport::{MessageFlooder, MessageProcessor, MessageSender, NetworkThreads};
use rsnano_utils::thread_pool::ThreadPool;

use super::lifecycle::Lifecycle;

/// Construction-only bundle of network collaborators used to wire up the
/// `NetworkSubsystem`. This is purely for composition; callers must not store
/// it on long-lived structs.
#[derive(Clone)]
pub struct NetworkWiring {
    pub network: Arc<RwLock<Network>>,
    pub tcp_listener: Arc<TcpListener>,
    pub peer_connector: Arc<PeerConnector>,
    pub network_threads: Arc<Mutex<NetworkThreads>>,
    pub message_processor: Arc<Mutex<MessageProcessor>>,
    pub message_sender: Arc<Mutex<MessageSender>>,
    pub message_flooder: Arc<Mutex<MessageFlooder>>,
    pub keepalive_publisher: Arc<KeepalivePublisher>,
    pub inbound_message_queue: Arc<InboundMessageQueue>,
    pub network_filter: Arc<NetworkFilter>,
    pub steady_clock: Arc<SteadyClock>,
}

/// Facade over network internals (listener, peer connector, filters, queues).
/// Concrete wiring will replace the placeholders during encapsulation work.
#[derive(Clone)]
pub struct NetworkSubsystem {
    network: Arc<RwLock<Network>>,
    tcp_listener: Arc<TcpListener>,
    peer_connector: Arc<PeerConnector>,
    network_threads: Arc<Mutex<NetworkThreads>>,
    message_processor: Arc<Mutex<MessageProcessor>>,
    message_sender: Arc<Mutex<MessageSender>>,
    message_flooder: Arc<Mutex<MessageFlooder>>,
    keepalive_publisher: Arc<KeepalivePublisher>,
    inbound_message_queue: Arc<InboundMessageQueue>,
    network_filter: Arc<NetworkFilter>,
    steady_clock: Arc<SteadyClock>,
    max_inbound_connections: usize,
    workers: Arc<ThreadPool>,
}

/// Test-only access to network internals.
#[derive(Clone)]
pub struct NetworkTestHandles {
    pub network: Arc<RwLock<Network>>,
    pub tcp_listener: Arc<TcpListener>,
    pub peer_connector: Arc<PeerConnector>,
    pub message_processor: Arc<Mutex<MessageProcessor>>,
    pub message_sender: Arc<Mutex<MessageSender>>,
    pub message_flooder: Arc<Mutex<MessageFlooder>>,
    pub keepalive_publisher: Arc<KeepalivePublisher>,
    pub inbound_message_queue: Arc<InboundMessageQueue>,
    pub network_filter: Arc<NetworkFilter>,
    pub steady_clock: Arc<SteadyClock>,
}

impl NetworkSubsystem {
    pub(crate) fn new(
        wiring: NetworkWiring,
        workers: Arc<ThreadPool>,
        max_inbound_connections: usize,
    ) -> Self {
        let NetworkWiring {
            network,
            tcp_listener,
            peer_connector,
            network_threads,
            message_processor,
            message_sender,
            message_flooder,
            keepalive_publisher,
            inbound_message_queue,
            network_filter,
            steady_clock,
        } = wiring;
        Self {
            network,
            tcp_listener,
            peer_connector,
            network_threads,
            message_processor,
            message_sender,
            message_flooder,
            keepalive_publisher,
            inbound_message_queue,
            network_filter,
            steady_clock,
            max_inbound_connections,
            workers,
        }
    }

    /// Returns the local listening endpoint for peer discovery.
    pub fn local_endpoint(&self) -> SocketAddrV6 {
        self.tcp_listener.local_address()
    }

    /// Current connected peer count.
    pub fn peer_count(&self) -> usize {
        self.network.read().unwrap().len()
    }

    /// Broadcast a keepalive to peers or connect when missing.
    pub async fn keepalive_or_connect(&self, address: String, port: u16) {
        self.keepalive_publisher
            .keepalive_or_connect(address, port)
            .await
    }

    /// Sorted realtime channels for diagnostics/telemetry.
    pub fn sorted_channels(&self) -> Vec<Arc<Channel>> {
        self.network.read().unwrap().sorted_channels()
    }

    /// Expose steady clock for timing sensitive callers.
    pub fn steady_clock(&self) -> Arc<SteadyClock> {
        self.steady_clock.clone()
    }

    /// Access to inbound queue for transport-level dispatchers.
    pub fn inbound_message_queue(&self) -> Arc<InboundMessageQueue> {
        self.inbound_message_queue.clone()
    }

    /// Lightweight reference to the network filter used by transport paths.
    pub fn network_filter(&self) -> Arc<NetworkFilter> {
        self.network_filter.clone()
    }

    fn start_internal(&self) {
        self.network_threads.lock().unwrap().start();
        if self.max_inbound_connections > 0 {
            self.tcp_listener.start();
        } else {
            warn!("Peering is disabled");
        }
        self.message_processor.lock().unwrap().start();
    }

    fn stop_internal(&self) {
        self.tcp_listener.stop();
        self.peer_connector.stop();
        self.message_processor.lock().unwrap().stop();
        self.network_threads.lock().unwrap().stop();
        self.workers.join();
    }

    pub fn start(&mut self) {
        self.start_internal();
    }

    pub fn stop(&mut self) {
        self.stop_internal();
    }

    pub fn stop_listeners(&self) {
        self.tcp_listener.stop();
        self.peer_connector.stop();
    }

    pub fn stop_threads(&self) {
        self.message_processor.lock().unwrap().stop();
        self.network_threads.lock().unwrap().stop();
    }

    /// **Legacy test access - technical debt.**
    ///
    /// This method exposes internal subsystem components for testing.
    /// It is marked hidden and should be avoided in new tests.
    /// Phase 5 will introduce behavioral test helpers to replace this pattern.
    #[doc(hidden)]
    pub fn test_handles(&self) -> NetworkTestHandles {
        NetworkTestHandles {
            network: self.network.clone(),
            tcp_listener: self.tcp_listener.clone(),
            peer_connector: self.peer_connector.clone(),
            message_processor: self.message_processor.clone(),
            message_sender: self.message_sender.clone(),
            message_flooder: self.message_flooder.clone(),
            keepalive_publisher: self.keepalive_publisher.clone(),
            inbound_message_queue: self.inbound_message_queue.clone(),
            network_filter: self.network_filter.clone(),
            steady_clock: self.steady_clock.clone(),
        }
    }
}

impl Lifecycle for NetworkSubsystem {
    fn start(&mut self) {
        self.start_internal();
    }

    fn stop(&mut self) {
        self.stop_internal();
    }
}
