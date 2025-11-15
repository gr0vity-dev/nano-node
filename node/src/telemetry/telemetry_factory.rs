use std::{
    sync::{Arc, Mutex, RwLock},
    time::SystemTime,
};

use rsnano_ledger::Ledger;
use rsnano_messages::{TelemetryData, TelemetryMaker};
use rsnano_network::{ChannelMode, Network};
use rsnano_nullable_clock::{SteadyClock, Timestamp};
use rsnano_types::{PrivateKey, Signature};

use crate::block_processing::UncheckedMap;

use super::{get_pre_release_version, rsnano_version};

/// Creates the telemetry data for this node
pub struct TelemetryFactory {
    pub ledger: Arc<Ledger>,
    pub network: Arc<RwLock<Network>>,
    pub node_id_key: PrivateKey,
    pub unchecked: Arc<Mutex<UncheckedMap>>,
    pub startup_time: Timestamp,
    pub clock: Arc<SteadyClock>,
}

impl TelemetryFactory {
    pub fn new_null() -> Self {
        Self {
            ledger: Ledger::new_null(rsnano_store_lmdb::null_ledger_store_factory()).into(),
            network: RwLock::new(Network::new_test_instance()).into(),
            node_id_key: PrivateKey::from(1),
            unchecked: Mutex::new(UncheckedMap::default()).into(),
            startup_time: Timestamp::new_test_instance(),
            clock: SteadyClock::new_null().into(),
        }
    }

    pub fn get_telemetry(&self) -> TelemetryData {
        let peer_count;
        let protocol_version;
        let bandwidth_cap;
        {
            let network = self.network.read().unwrap();
            peer_count = network.count_by_mode(ChannelMode::Realtime) as u32;
            protocol_version = network.protocol_info().version_using;
            bandwidth_cap = network.bandwidth_limit() as u64;
        }

        let version = rsnano_version();
        let unchecked_count = self.unchecked.lock().unwrap().len() as u64;

        let mut telemetry_data = TelemetryData {
            node_id: self.node_id_key.public_key().into(),
            block_count: self.ledger.block_count(),
            cemented_count: self.ledger.confirmed_count(),
            bandwidth_cap,
            protocol_version,
            uptime: self.startup_time.elapsed(self.clock.now()).as_secs(),
            unchecked_count,
            genesis_block: self.ledger.genesis().hash(),
            peer_count,
            account_count: self.ledger.account_count(),
            major_version: version.major as u8,
            minor_version: version.minor as u8,
            patch_version: version.patch as u8,
            pre_release_version: get_pre_release_version(&version),
            maker: TelemetryMaker::RsNano as u8,
            timestamp: SystemTime::now(),
            active_difficulty: self.ledger.work_thresholds().threshold_base(),
            unknown_data: Vec::new(),
            signature: Signature::default(),
        };
        // Make sure this is the final operation!
        telemetry_data.sign(&self.node_id_key).unwrap();
        telemetry_data
    }
}
