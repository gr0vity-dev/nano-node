use super::NodeToml;
use crate::{cementation::ConfirmingSetConfig, config::NodeConfig};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Deserialize, Serialize)]
pub struct ConfirmingSetToml {
    pub batch_size: Option<usize>,
    pub max_blocks: Option<usize>,
    pub max_queued_notifications: Option<usize>,
    pub max_deferred: Option<usize>,
    /// Milliseconds
    pub deferred_age_cutoff: Option<u64>,
}

impl ConfirmingSetConfig {
    pub(crate) fn merge_toml(&mut self, root: &NodeToml) {
        let Some(toml) = &root.confirming_set else {
            return;
        };

        if let Some(batch_size) = toml.batch_size {
            self.batch_size = batch_size;
        }
        if let Some(max_blocks) = toml.max_blocks {
            self.max_blocks = max_blocks;
        }
        if let Some(max_queued_notifications) = toml.max_queued_notifications {
            self.max_queued_notifications = max_queued_notifications;
        }
        if let Some(max_deferred) = toml.max_deferred {
            self.max_deferred = max_deferred;
        }
        if let Some(ms) = toml.deferred_age_cutoff {
            self.deferred_age_cutoff = Duration::from_millis(ms);
        }
    }
}

impl From<&NodeConfig> for ConfirmingSetToml {
    fn from(value: &NodeConfig) -> Self {
        Self {
            batch_size: Some(value.confirming_set.batch_size),
            max_blocks: Some(value.confirming_set.max_blocks),
            max_queued_notifications: Some(value.confirming_set.max_queued_notifications),
            max_deferred: Some(value.confirming_set.max_deferred),
            deferred_age_cutoff: Some(value.confirming_set.deferred_age_cutoff.as_millis() as u64),
        }
    }
}
