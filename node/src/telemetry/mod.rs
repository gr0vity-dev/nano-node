mod telemetry_factory;

use rsnano_ledger::DEV_GENESIS_HASH;
pub use telemetry_factory::TelemetryFactory;

use std::{
    cmp::min,
    collections::{HashMap, VecDeque},
    mem::size_of,
    net::SocketAddrV6,
    sync::{Arc, Condvar, Mutex, RwLock},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use rsnano_messages::{Message, TelemetryAck, TelemetryData};
use rsnano_network::{Channel, ChannelId, DeadChannelCleanupStep, Network, TrafficType};
use rsnano_nullable_clock::SteadyClock;
use rsnano_types::BlockHash;
use rsnano_utils::container_info::{ContainerInfo, ContainerInfoProvider};
use rsnano_utils::stats::{DetailType, StatType, Stats};

use crate::{
    config::{DEV_NETWORK_PARAMS, NetworkParams},
    transport::MessageSender,
};

#[derive(Clone, Debug, PartialEq)]
pub struct TelemetrySnapshot {
    pub endpoint: SocketAddrV6,
    pub data: TelemetryData,
}

/**
 * This class periodically broadcasts and requests telemetry from peers.
 * Those intervals are configurable via `telemetry_request_interval` & `telemetry_broadcast_interval` network constants
 * Telemetry datas are only removed after becoming stale (configurable via `telemetry_cache_cutoff` network constant), so peer data will still be available for a short period after that peer is disconnected
 *
 * Broadcasts can be disabled via `disable_providing_telemetry_metrics` node flag
 *
 */
pub struct Telemetry {
    telemetry_factory: TelemetryFactory,
    config: TelementryConfig,
    stats: Arc<Stats>,
    thread: Mutex<Option<JoinHandle<()>>>,
    condition: Condvar,
    mutex: Mutex<TelemetryImpl>,
    network_params: NetworkParams,
    network: Arc<RwLock<Network>>,
    message_sender: Mutex<MessageSender>,
    pub startup_time: Instant,
    telemetry_processed_callbacks:
        Mutex<Vec<Box<dyn Fn(&TelemetryData, &SocketAddrV6) + Send + Sync>>>,
    clock: Arc<SteadyClock>,
    genesis_hash: BlockHash,
}

impl Telemetry {
    const MAX_SIZE: usize = 1024;

    pub(crate) fn new(
        telemetry_factory: TelemetryFactory,
        config: TelementryConfig,
        stats: Arc<Stats>,
        genesis_hash: BlockHash,
        network_params: NetworkParams,
        network: Arc<RwLock<Network>>,
        message_sender: MessageSender,
        clock: Arc<SteadyClock>,
    ) -> Self {
        Self {
            telemetry_factory,
            config,
            stats,
            network_params,
            network,
            message_sender: Mutex::new(message_sender),
            thread: Mutex::new(None),
            condition: Condvar::new(),
            mutex: Mutex::new(TelemetryImpl {
                stopped: false,
                telemetries: Default::default(),
                last_broadcast: None,
            }),
            telemetry_processed_callbacks: Mutex::new(Vec::new()),
            startup_time: Instant::now(),
            clock,
            genesis_hash,
        }
    }

    pub fn new_null() -> Self {
        Telemetry::new(
            TelemetryFactory::new_null(),
            TelementryConfig::default(),
            Stats::default().into(),
            *DEV_GENESIS_HASH,
            DEV_NETWORK_PARAMS.clone(),
            RwLock::new(Network::new_test_instance()).into(),
            MessageSender::new_null(),
            SteadyClock::new_null().into(),
        )
    }

    pub fn stop(&self) {
        self.mutex.lock().unwrap().stopped = true;
        self.condition.notify_all();
        let handle = self.thread.lock().unwrap().take();
        if let Some(handle) = handle {
            handle.join().unwrap();
        }
    }

    pub fn on_telemetry_processed(
        &self,
        f: Box<dyn Fn(&TelemetryData, &SocketAddrV6) + Send + Sync>,
    ) {
        self.telemetry_processed_callbacks.lock().unwrap().push(f);
    }

    fn verify(&self, telemetry: &TelemetryAck, channel: &Channel) -> bool {
        let Some(data) = &telemetry.0 else {
            self.stats
                .inc(StatType::Telemetry, DetailType::EmptyPayload);
            return false;
        };

        // Check if telemetry node id matches channel node id
        if Some(data.node_id) != channel.node_id() {
            self.stats
                .inc(StatType::Telemetry, DetailType::NodeIdMismatch);
            return false;
        }

        // Check whether data is signed by node id presented in telemetry message
        if !data.validate_signature() {
            self.stats
                .inc(StatType::Telemetry, DetailType::InvalidSignature);
            return false;
        }

        if data.genesis_block != self.genesis_hash {
            self.network
                .write()
                .unwrap()
                .peer_misbehaved(channel.channel_id(), self.clock.now());

            self.stats
                .inc(StatType::Telemetry, DetailType::GenesisMismatch);
            return false;
        }

        true // Telemetry is OK
    }

    /// Process telemetry message from network
    pub fn process(&self, telemetry: &TelemetryAck, channel: &Channel) {
        if !self.verify(telemetry, channel) {
            return;
        }
        let data = telemetry.0.as_ref().unwrap();

        let mut guard = self.mutex.lock().unwrap();
        let peer_addr = channel.peer_addr();

        if let Some(entry) = guard.telemetries.get_mut(channel.channel_id()) {
            self.stats.inc(StatType::Telemetry, DetailType::Update);
            entry.data = data.clone();
            entry.last_updated = Instant::now();
        } else {
            self.stats.inc(StatType::Telemetry, DetailType::Insert);
            guard.telemetries.push_back(Entry {
                channel_id: channel.channel_id(),
                endpoint: peer_addr,
                data: data.clone(),
                last_updated: Instant::now(),
            });

            if guard.telemetries.len() > Self::MAX_SIZE {
                self.stats.inc(StatType::Telemetry, DetailType::Overfill);
                guard.telemetries.pop_front(); // Erase oldest entry
            }
        }

        drop(guard);

        {
            let callbacks = self.telemetry_processed_callbacks.lock().unwrap();
            for callback in callbacks.iter() {
                (callback)(data, &peer_addr);
            }
        }

        self.stats.inc(StatType::Telemetry, DetailType::Process);
    }

    pub fn len(&self) -> usize {
        self.mutex.lock().unwrap().telemetries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn broadcast_predicate(&self, data: &TelemetryImpl) -> bool {
        if !self.config.enable_ongoing_broadcasts {
            return false;
        }

        data.last_broadcast.is_none()
            || data.last_broadcast.unwrap().elapsed()
                >= Duration::from_millis(
                    self.network_params.network.telemetry_broadcast_interval_ms as u64,
                )
    }

    fn run(&self) {
        let mut guard = self.mutex.lock().unwrap();
        while !guard.stopped {
            self.stats.inc(StatType::Telemetry, DetailType::Loop);
            self.cleanup(&mut guard);

            if self.broadcast_predicate(&guard) {
                drop(guard);

                self.run_broadcasts();

                guard = self.mutex.lock().unwrap();
                guard.last_broadcast = Some(Instant::now());
            }

            let wait_duration = min(
                self.network_params.network.telemetry_request_interval_ms,
                self.network_params.network.telemetry_broadcast_interval_ms / 2,
            );
            guard = self
                .condition
                .wait_timeout(guard, Duration::from_millis(wait_duration as u64))
                .unwrap()
                .0
        }
    }

    fn run_broadcasts(&self) {
        let telemetry = self.local_telemetry();
        let channels = self
            .network
            .read()
            .unwrap()
            .shuffled_channels(TrafficType::Telemetry);
        let message = Message::TelemetryAck(TelemetryAck(Some(telemetry)));
        for channel in channels {
            self.broadcast(&channel, &message);
        }
    }

    fn broadcast(&self, channel: &Channel, message: &Message) {
        self.stats.inc(StatType::Telemetry, DetailType::Broadcast);
        self.message_sender
            .lock()
            .unwrap()
            .try_send(channel, message, TrafficType::Telemetry);
    }

    fn cleanup(&self, data: &mut TelemetryImpl) {
        data.telemetries.retain(|entry| {
            // Remove if telemetry data is stale
            if self.has_timed_out(entry) {
                self.stats
                    .inc(StatType::Telemetry, DetailType::CleanupOutdated);
                false // Erase
            } else {
                true // Retain
            }
        })
    }

    fn has_timed_out(&self, entry: &Entry) -> bool {
        entry.last_updated.elapsed()
            > Duration::from_millis(self.network_params.network.telemetry_cache_cutoff_ms as u64)
    }

    /// Returns telemetry for selected endpoint
    pub fn get_telemetry(&self, endpoint: &SocketAddrV6) -> Option<TelemetryData> {
        let guard = self.mutex.lock().unwrap();
        if let Some(entry) = guard.telemetries.get_by_endpoint(endpoint)
            && !self.has_timed_out(entry)
        {
            return Some(entry.data.clone());
        }
        None
    }

    pub fn get_all_telemetries(&self) -> Vec<TelemetrySnapshot> {
        let guard = self.mutex.lock().unwrap();
        guard
            .telemetries
            .iter()
            .filter(|entry| !self.has_timed_out(entry))
            .map(|entry| TelemetrySnapshot {
                endpoint: entry.endpoint,
                data: entry.data.clone(),
            })
            .collect()
    }

    pub fn local_telemetry(&self) -> TelemetryData {
        self.telemetry_factory.get_telemetry()
    }
}

build_info::build_info!(fn build_info);

pub fn rsnano_version_string() -> String {
    let version = &build_info().crate_info.version;
    if version.pre.is_empty() {
        format!("RsNano V{}.{}", version.major, version.minor)
    } else {
        format!(
            "RsNano V{}.{}-{}",
            version.major, version.minor, version.pre
        )
    }
}

pub fn rsnano_build_info() -> String {
    let info = build_info();
    format!(
        "built with {} ({}) at {} from {}",
        info.compiler,
        info.profile,
        info.timestamp,
        info.version_control
            .as_ref()
            .map(|i| i.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    )
}

pub use build_info::semver::Version;

pub fn rsnano_version() -> Version {
    build_info().crate_info.version.clone()
}

pub fn get_pre_release_version(v: &Version) -> u8 {
    if v.pre.is_empty() {
        0
    } else {
        v.pre
            .split('.')
            .nth(1)
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(99)
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        // Thread must be stopped before destruction
        debug_assert!(self.thread.lock().unwrap().is_none());
    }
}

impl ContainerInfoProvider for Telemetry {
    fn container_info(&self) -> ContainerInfo {
        let guard = self.mutex.lock().unwrap();
        [(
            "telemetries",
            guard.telemetries.len(),
            OrderedTelemetries::ELEMENT_SIZE,
        )]
        .into()
    }
}

pub struct TelementryConfig {
    pub enable_ongoing_broadcasts: bool,
}

impl Default for TelementryConfig {
    fn default() -> Self {
        Self {
            enable_ongoing_broadcasts: true,
        }
    }
}

pub trait TelementryExt {
    fn start(&self);
}

struct TelemetryImpl {
    stopped: bool,
    telemetries: OrderedTelemetries,
    last_broadcast: Option<Instant>,
}

impl TelementryExt for Arc<Telemetry> {
    fn start(&self) {
        debug_assert!(self.thread.lock().unwrap().is_none());
        let self_l = Arc::clone(self);
        *self.thread.lock().unwrap() = Some(
            std::thread::Builder::new()
                .name("Telemetry".to_string())
                .spawn(move || {
                    self_l.run();
                })
                .unwrap(),
        );
    }
}

struct Entry {
    channel_id: ChannelId,
    endpoint: SocketAddrV6,
    data: TelemetryData,
    last_updated: Instant,
}

#[derive(Default)]
struct OrderedTelemetries {
    by_channel_id: HashMap<ChannelId, Entry>,
    by_endpoint: HashMap<SocketAddrV6, ChannelId>,
    sequenced: VecDeque<ChannelId>,
}

impl OrderedTelemetries {
    pub const ELEMENT_SIZE: usize = size_of::<Entry>() + size_of::<SocketAddrV6>() * 2;
    fn len(&self) -> usize {
        self.sequenced.len()
    }

    fn push_back(&mut self, entry: Entry) {
        let channel_id = entry.channel_id;
        let endpoint = entry.endpoint;
        if let Some(old) = self.by_channel_id.insert(channel_id, entry) {
            if old.endpoint != endpoint {
                // This should never be reached
                self.by_endpoint.remove(&old.endpoint);
            }
            // already in sequenced
        } else {
            self.sequenced.push_back(channel_id);
        }
        self.by_endpoint.insert(endpoint, channel_id);
    }

    fn get_by_endpoint(&self, entpoint: &SocketAddrV6) -> Option<&Entry> {
        let channel_id = self.by_endpoint.get(entpoint)?;
        self.by_channel_id.get(channel_id)
    }

    fn get_mut(&mut self, channel_id: ChannelId) -> Option<&mut Entry> {
        self.by_channel_id.get_mut(&channel_id)
    }

    fn remove(&mut self, channel_id: ChannelId) {
        if let Some(entry) = self.by_channel_id.remove(&channel_id) {
            self.by_endpoint.remove(&entry.endpoint);
            self.sequenced.retain(|i| *i != channel_id);
        }
    }

    fn pop_front(&mut self) {
        if let Some(channel_id) = self.sequenced.pop_front()
            && let Some(entry) = self.by_channel_id.remove(&channel_id)
        {
            self.by_endpoint.remove(&entry.endpoint);
        }
    }

    fn retain(&mut self, mut f: impl FnMut(&Entry) -> bool) {
        self.by_channel_id.retain(|channel_id, entry| {
            let retain = f(entry);
            if !retain {
                self.sequenced.retain(|i| i != channel_id);
                self.by_endpoint.remove(&entry.endpoint);
            }
            retain
        })
    }

    fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.by_channel_id.values()
    }
}

impl DeadChannelCleanupStep for Telemetry {
    fn clean_up_dead_channels(&self, dead_channel_ids: &[ChannelId]) {
        let mut guard = self.mutex.lock().unwrap();
        for channel_id in dead_channel_ids {
            guard.telemetries.remove(*channel_id);
        }
    }
}

pub struct TelemetryDeadChannelCleanup(pub Arc<Telemetry>);

impl DeadChannelCleanupStep for TelemetryDeadChannelCleanup {
    fn clean_up_dead_channels(&self, dead_channel_ids: &[ChannelId]) {
        self.0.clean_up_dead_channels(dead_channel_ids);
    }
}
