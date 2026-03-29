use std::{
    collections::HashSet,
    mem::size_of,
    ops::Deref,
    sync::{Arc, Mutex},
    thread::JoinHandle,
};

use strum_macros::{EnumCount, EnumIter};
use tracing::debug;

use rsnano_ledger::RepWeightCache;
use rsnano_types::PublicKey;
use rsnano_utils::{
    CancellationToken,
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{DetailType, Direction, StatType, Stats},
    ticker::Tickable,
};

use crate::representatives::{OnlineReps, VoteQuorumPreparer};

// Higher number means higher priority
#[derive(Copy, Clone, PartialOrd, Ord, PartialEq, Eq, EnumIter, Hash, Debug, EnumCount)]
pub enum RepTier {
    /// Not a principal representatives
    None,
    /// (0.1-1%) of online stake
    Tier1,
    /// (1-5%) of online stake
    Tier2,
    /// (> 5%) of online stake
    Tier3,
}

impl RepTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            RepTier::None => "none",
            RepTier::Tier1 => "tier1",
            RepTier::Tier2 => "tier2",
            RepTier::Tier3 => "tier3",
        }
    }
}

impl From<RepTier> for DetailType {
    fn from(value: RepTier) -> Self {
        match value {
            RepTier::None => DetailType::None,
            RepTier::Tier1 => DetailType::Tier1,
            RepTier::Tier2 => DetailType::Tier2,
            RepTier::Tier3 => DetailType::Tier3,
        }
    }
}

#[derive(Default, Clone)]
pub struct RepTiers {
    /// 0.1% or above
    pub tier1: HashSet<PublicKey>,
    /// 1% or above
    pub tier2: HashSet<PublicKey>,
    /// 5% or above
    pub tier3: HashSet<PublicKey>,
}

impl RepTiers {
    pub fn tier(&self, representative: &PublicKey) -> RepTier {
        if self.tier3.contains(representative) {
            RepTier::Tier3
        } else if self.tier2.contains(representative) {
            RepTier::Tier2
        } else if self.tier1.contains(representative) {
            RepTier::Tier1
        } else {
            RepTier::None
        }
    }
}

pub struct CurrentRepTiers(Mutex<RepTiers>);

impl CurrentRepTiers {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for CurrentRepTiers {
    fn default() -> Self {
        Self(Mutex::new(RepTiers::default()))
    }
}

impl RepTiersConsumer for CurrentRepTiers {
    fn update_rep_tiers(&self, new_tiers: RepTiers) {
        *self.0.lock().unwrap() = new_tiers;
    }
}

impl Deref for CurrentRepTiers {
    type Target = Mutex<RepTiers>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ContainerInfoProvider for CurrentRepTiers {
    fn container_info(&self) -> ContainerInfo {
        let tiers = self.lock().unwrap();
        [
            ("tier_1", tiers.tier1.len(), size_of::<PublicKey>()),
            ("tier_2", tiers.tier2.len(), size_of::<PublicKey>()),
            ("tier_3", tiers.tier3.len(), size_of::<PublicKey>()),
        ]
        .into()
    }
}

pub trait RepTiersConsumer {
    fn update_rep_tiers(&self, new_tiers: RepTiers);
}

impl<T> RepTiersConsumer for Arc<T>
where
    T: RepTiersConsumer,
{
    fn update_rep_tiers(&self, new_tiers: RepTiers) {
        self.as_ref().update_rep_tiers(new_tiers);
    }
}

pub struct RepTiersCalculator {
    thread: Mutex<Option<JoinHandle<()>>>,
    rep_weights: Arc<RepWeightCache>,
    quorum_preparer: Arc<VoteQuorumPreparer>,
    stats: Arc<Stats>,
    consumers: Vec<Box<dyn RepTiersConsumer + Send + Sync>>,
}

impl RepTiersCalculator {
    pub fn new(
        rep_weights: Arc<RepWeightCache>,
        _online_reps: Arc<Mutex<OnlineReps>>,
        quorum_preparer: Arc<VoteQuorumPreparer>,
        stats: Arc<Stats>,
    ) -> Self {
        Self {
            thread: Mutex::new(None),
            rep_weights,
            quorum_preparer,
            stats,
            consumers: Vec::new(),
        }
    }

    pub fn add_tiers_consumer(&mut self, consumer: impl RepTiersConsumer + Send + Sync + 'static) {
        self.consumers.push(Box::new(consumer));
    }

    fn calculate_tiers(&mut self) {
        self.stats.inc(StatType::RepTiers, DetailType::Loop);
        let trended = self.quorum_preparer.trended_or_minimum_weight();
        let mut new_tier1 = HashSet::new();
        let mut new_tier2 = HashSet::new();
        let mut new_tier3 = HashSet::new();
        let mut ignored = 0;
        let reps_count;
        {
            let rep_weights = self.rep_weights.read();
            reps_count = rep_weights.len();
            for (&representative, &weight) in rep_weights.iter() {
                if weight > trended / 1000 {
                    // 0.1% or above (level 1)
                    new_tier1.insert(representative);
                    if weight > trended / 100 {
                        // 1% or above (level 2)
                        new_tier2.insert(representative);
                        if weight > trended / 20 {
                            // 5% or above (level 3)
                            new_tier3.insert(representative);
                        }
                    }
                } else {
                    ignored += 1;
                }
            }
        }

        self.stats.add_dir(
            StatType::RepTiers,
            DetailType::Processed,
            Direction::In,
            reps_count as u64,
        );

        self.stats.add_dir(
            StatType::RepTiers,
            DetailType::Ignored,
            Direction::In,
            ignored,
        );

        debug!(
            "Representative tiers updated, tier 1: {}, tier 2: {}, tier 3: {} ({} ignored)",
            new_tier1.len(),
            new_tier2.len(),
            new_tier3.len(),
            ignored
        );

        let new_rep_tiers = RepTiers {
            tier1: new_tier1,
            tier2: new_tier2,
            tier3: new_tier3,
        };

        for consumer in &self.consumers {
            consumer.update_rep_tiers(new_rep_tiers.clone());
        }

        self.stats.inc(StatType::RepTiers, DetailType::Updated);
    }
}

impl Drop for RepTiersCalculator {
    fn drop(&mut self) {
        // Thread must be stopped before destruction
        debug_assert!(self.thread.lock().unwrap().is_none());
    }
}

impl Tickable for RepTiersCalculator {
    fn tick(&mut self, _cancel_token: &CancellationToken) {
        self.calculate_tiers();
    }
}
