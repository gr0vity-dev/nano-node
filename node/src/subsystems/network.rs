//! NetworkSubsystem manages peer connections, message routing, and transport lifecycle.
//! Production APIs expose connection info as DTOs; raw channels/queues/filters are test-only.

use std::{
    net::SocketAddrV6,
    sync::{Arc, Mutex, RwLock},
};

use anyhow::{Result, anyhow};
use rsnano_messages::{Message, NetworkFilter};
#[cfg(any(test, feature = "test_support"))]
use rsnano_network::Channel;
use rsnano_network::{
    ChannelDirection, ChannelId, ChannelMode, Network, PeerConnector, TcpListener, TcpListenerExt,
};
use rsnano_network_protocol::InboundMessageQueue;
use rsnano_nullable_clock::{SteadyClock, Timestamp};
use rsnano_types::NodeId;
use tracing::warn;

use crate::transport::keepalive::KeepalivePublisher;
use crate::transport::{MessageFlooder, MessageProcessor, MessageSender, NetworkThreads};
use rsnano_utils::thread_pool::ThreadPool;

use super::lifecycle::Lifecycle;
use crate::services::NetworkServices;

/// Construction-only bundle of network collaborators used to wire up the
/// `NetworkSubsystem`. This is purely for composition; callers must not store
/// it on long-lived structs.
#[derive(Clone)]
pub(crate) struct NetworkWiring {
    pub(crate) network: Arc<RwLock<Network>>,
    pub(crate) tcp_listener: Arc<TcpListener>,
    pub(crate) peer_connector: Arc<PeerConnector>,
    pub(crate) network_threads: Arc<Mutex<NetworkThreads>>,
    pub(crate) message_processor: Arc<Mutex<MessageProcessor>>,
    pub(crate) message_sender: Arc<Mutex<MessageSender>>,
    pub(crate) message_flooder: Arc<Mutex<MessageFlooder>>,
    pub(crate) keepalive_publisher: Arc<KeepalivePublisher>,
    pub(crate) inbound_message_queue: Arc<InboundMessageQueue>,
    pub(crate) network_filter: Arc<NetworkFilter>,
    pub(crate) steady_clock: Arc<SteadyClock>,
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

#[derive(Clone, Debug)]
pub struct ChannelInfo {
    pub channel_id: ChannelId,
    pub endpoint: SocketAddrV6,
    pub peering_endpoint: SocketAddrV6,
    pub direction: ChannelDirection,
    pub protocol_version: u8,
    pub node_id: Option<NodeId>,
    pub score: i32,
    pub last_packet_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct FilterDiagnostics {
    pub size: usize,
    pub age_cutoff: u64,
    pub current_epoch: u64,
}

/// Test-only access to network internals.
#[cfg(any(test, feature = "test_support"))]
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
    #[cfg(any(test, feature = "test_support"))]
    #[deprecated(note = "Use channel_infos() for production code")]
    pub fn sorted_channels(&self) -> Vec<Arc<Channel>> {
        self.network.read().unwrap().sorted_channels()
    }

    /// Returns DTOs describing realtime channels without exposing internals.
    pub fn channel_infos(&self) -> Vec<ChannelInfo> {
        let now = self.steady_clock.now();
        self.network
            .read()
            .unwrap()
            .sorted_channels()
            .into_iter()
            .map(|channel| {
                let last_activity = channel.last_activity();
                let last_packet_ms =
                    now.millis().saturating_sub(last_activity.millis()).max(0) as u64;
                ChannelInfo {
                    channel_id: channel.channel_id(),
                    endpoint: channel.peer_addr(),
                    peering_endpoint: channel.peering_addr_or_peer_addr(),
                    direction: channel.direction(),
                    protocol_version: channel.protocol_version(),
                    node_id: channel.node_id(),
                    score: 0,
                    last_packet_ms,
                }
            })
            .collect()
    }

    /// Current steady clock timestamp.
    pub fn now(&self) -> Timestamp {
        self.steady_clock.now()
    }

    pub fn steady_now(&self) -> Timestamp {
        self.steady_clock.now()
    }

    pub fn inbound_queue_len(&self) -> usize {
        self.inbound_message_queue.size()
    }

    pub fn enqueue_inbound(&self, message: Message, endpoint: SocketAddrV6) -> Result<()> {
        let channel = {
            if let Some(channel) = self
                .network
                .read()
                .unwrap()
                .find_realtime_channel_by_remote_addr(&endpoint)
            {
                channel.clone()
            } else {
                let local = self.tcp_listener.local_address();
                let now = self.steady_clock.now();
                let (channel, _) = self
                    .network
                    .write()
                    .unwrap()
                    .add(local, endpoint, ChannelDirection::Inbound, now)
                    .map_err(|e| anyhow!(e.to_string()))?;
                channel.set_mode(ChannelMode::Realtime);
                channel
            }
        };

        if self.inbound_message_queue.put(message, channel) {
            Ok(())
        } else {
            Err(anyhow!("Inbound queue overfilled"))
        }
    }

    pub fn connect_test_peer(&self, endpoint: SocketAddrV6, node_id: Option<NodeId>) -> ChannelId {
        let local = self.tcp_listener.local_address();
        let now = self.steady_clock.now();
        let (channel, _) = self
            .network
            .write()
            .unwrap()
            .add(local, endpoint, ChannelDirection::Outbound, now)
            .expect("test peer connection should succeed");
        channel.set_mode(ChannelMode::Realtime);
        #[cfg(any(test, feature = "test_support"))]
        channel.reopen_for_tests();
        self.network
            .write()
            .unwrap()
            .upgrade_to_realtime_connection(channel.channel_id(), node_id.unwrap_or_default());
        channel.channel_id()
    }

    pub fn set_channel_node_id(&self, channel_id: ChannelId, node_id: NodeId) {
        self.network
            .write()
            .unwrap()
            .set_node_id(channel_id, node_id);
    }

    pub fn filter_counts(&self) -> FilterDiagnostics {
        FilterDiagnostics {
            size: self.network_filter.capacity(),
            age_cutoff: self.network_filter.age_cutoff,
            current_epoch: self.network_filter.current_epoch(),
        }
    }

    /// Access to inbound queue for transport-level dispatchers.
    #[cfg(any(test, feature = "test_support"))]
    pub fn inbound_message_queue(&self) -> Arc<InboundMessageQueue> {
        self.inbound_message_queue.clone()
    }

    /// Lightweight reference to the network filter used by transport paths.
    #[cfg(any(test, feature = "test_support"))]
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

    pub fn services(&self) -> NetworkServices {
        NetworkServices::new(self.clone())
    }

    /// **Legacy test access - technical debt.**
    ///
    /// This method exposes internal subsystem components for testing.
    /// It is marked hidden and should be avoided in new tests.
    /// Phase 5 will introduce behavioral test helpers to replace this pattern.
    #[cfg(any(test, feature = "test_support"))]
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
