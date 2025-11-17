use crate::config::NodeConfig;
use rsnano_types::Peer;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct ExperimentalToml {
    pub secondary_work_peers: Option<Vec<String>>,
    pub rocksdb_optimizations_enabled: Option<bool>,
}

impl NodeConfig {
    pub fn merge_experimental_toml(&mut self, toml: &ExperimentalToml) {
        if let Some(peers) = &toml.secondary_work_peers {
            self.secondary_work_peers = peers
                .iter()
                .map(|string| Peer::from_str(&string).expect("Invalid secondary work peer"))
                .collect();
        }
        if let Some(enabled) = toml.rocksdb_optimizations_enabled {
            self.rocksdb_optimizations_enabled = enabled;
        }
    }
}

impl From<&NodeConfig> for ExperimentalToml {
    fn from(config: &NodeConfig) -> Self {
        Self {
            secondary_work_peers: Some(
                config
                    .secondary_work_peers
                    .iter()
                    .map(|peer| peer.to_string())
                    .collect(),
            ),
            rocksdb_optimizations_enabled: Some(config.rocksdb_optimizations_enabled),
        }
    }
}
