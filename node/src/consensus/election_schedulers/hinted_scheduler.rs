use std::{
    cmp::min,
    collections::{BTreeMap, HashMap},
    mem::size_of,
    time::{Duration, Instant},
};

use rsnano_types::{Amount, BlockHash};
use rsnano_utils::container_info::ContainerInfo;

use crate::representatives::OnlineReps;

#[derive(Clone, Debug, PartialEq)]
pub struct HintedSchedulerConfig {
    pub check_interval: Duration,
    pub block_cooldown: Duration,
    pub hinting_threshold_percent: u32,
    pub vacancy_threshold_percent: u32,
    /// Limit of hinted elections as percentage of `active_elections_size`
    pub hinted_limit_percentage: usize,
}

impl HintedSchedulerConfig {
    pub fn default_for_dev_network() -> Self {
        Self {
            check_interval: Duration::from_millis(100),
            block_cooldown: Duration::from_millis(100),
            ..Default::default()
        }
    }
}

impl Default for HintedSchedulerConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_millis(1000),
            block_cooldown: Duration::from_millis(5000),
            hinting_threshold_percent: 10,
            vacancy_threshold_percent: 20,
            hinted_limit_percentage: 20,
        }
    }
}

pub(crate) struct HintedSchedulerState {
    config: HintedSchedulerConfig,
    cooldowns: OrderedCooldowns,
    max_elections: usize,
    notification_threshold: usize,
}

impl HintedSchedulerState {
    pub(crate) fn new(config: HintedSchedulerConfig, max_active_elections: usize) -> Self {
        let max_elections = max_active_elections * config.hinted_limit_percentage / 100;
        let notification_threshold =
            max_elections * config.vacancy_threshold_percent as usize / 100;

        Self {
            config,
            cooldowns: OrderedCooldowns::new(),
            max_elections,
            notification_threshold,
        }
    }

    pub(crate) fn check_interval(&self) -> Duration {
        self.config.check_interval
    }

    pub(crate) fn max_elections(&self) -> usize {
        self.max_elections
    }

    pub(crate) fn should_run(&self, hinted_count: usize, aec_vacancy: i64) -> bool {
        self.vacancy(hinted_count, aec_vacancy) > 0
    }

    pub(crate) fn should_notify(&self, hinted_count: usize, aec_vacancy: i64) -> bool {
        self.vacancy(hinted_count, aec_vacancy) >= self.notification_threshold as i64
    }

    pub(crate) fn tally_threshold(&self, online_reps: &OnlineReps) -> Amount {
        (online_reps.trended_or_minimum_weight() / 100)
            * self.config.hinting_threshold_percent as u128
    }

    pub(crate) fn final_tally_threshold(&self, online_reps: &OnlineReps) -> Amount {
        online_reps.quorum_delta()
    }

    pub(crate) fn cooldown(&mut self, hash: BlockHash) -> bool {
        let now = Instant::now();
        if let Some(timeout) = self.cooldowns.get(&hash) {
            if *timeout > now {
                return true;
            }
            self.cooldowns.remove(&hash);
        }

        self.cooldowns
            .insert(hash, now + self.config.block_cooldown);
        self.cooldowns.trim(now);
        false
    }

    pub(crate) fn container_info(&self) -> ContainerInfo {
        [(
            "cooldowns",
            self.cooldowns.len(),
            (size_of::<BlockHash>() + size_of::<Instant>()) * 2,
        )]
        .into()
    }

    fn vacancy(&self, hinted_count: usize, aec_vacancy: i64) -> i64 {
        let hinted_vacancy = self.max_elections as i64 - hinted_count as i64;
        min(hinted_vacancy, aec_vacancy)
    }
}

struct OrderedCooldowns {
    by_hash: HashMap<BlockHash, Instant>,
    by_time: BTreeMap<Instant, Vec<BlockHash>>,
}

impl OrderedCooldowns {
    fn new() -> Self {
        Self {
            by_hash: HashMap::new(),
            by_time: BTreeMap::new(),
        }
    }

    fn insert(&mut self, hash: BlockHash, timeout: Instant) {
        if let Some(old_timeout) = self.by_hash.insert(hash, timeout) {
            self.remove_timeout_entry(&hash, old_timeout);
        }
        self.by_time.entry(timeout).or_default().push(hash);
    }

    fn get(&self, hash: &BlockHash) -> Option<&Instant> {
        self.by_hash.get(hash)
    }

    fn remove(&mut self, hash: &BlockHash) {
        if let Some(timeout) = self.by_hash.remove(hash) {
            self.remove_timeout_entry(hash, timeout);
        }
    }

    fn remove_timeout_entry(&mut self, hash: &BlockHash, timeout: Instant) {
        if let Some(hashes) = self.by_time.get_mut(&timeout) {
            if hashes.len() == 1 {
                self.by_time.remove(&timeout);
            } else {
                hashes.retain(|h| h != hash);
            }
        }
    }

    fn trim(&mut self, now: Instant) {
        while let Some(entry) = self.by_time.first_entry() {
            if *entry.key() <= now {
                let hashes = entry.remove();
                for hash in hashes {
                    self.by_hash.remove(&hash);
                }
            } else {
                break;
            }
        }
    }

    fn len(&self) -> usize {
        self.by_hash.len()
    }
}
