use super::stopped_counter::StoppedCounter;
use crate::consensus::{
    active_elections::AEC_STAT_KEY,
    election::{ConfirmationType, Election, ElectionBehavior},
};
use rsnano_utils::stats::{StatsCollection, StatsSource};
use strum::{EnumCount, IntoEnumIterator};

#[derive(Default)]
pub(crate) struct AecStats {
    stopped_counter: StoppedCounter,
    pub ticked: u64,
    pub conflicts: u64,
    pub started: u64,
    pub started_by_behavor: [u64; ElectionBehavior::COUNT],
    pub block_confirmations: [usize; ConfirmationType::COUNT],
}

impl AecStats {
    pub fn started(&mut self, behavior: ElectionBehavior) {
        self.started += 1;
        self.started_by_behavor[behavior as usize] += 1;
    }

    pub fn stopped(&mut self, election: &Election) {
        self.stopped_counter.stopped(election);
    }
}

impl StatsSource for AecStats {
    fn collect_stats(&self, result: &mut StatsCollection) {
        result.insert(AEC_STAT_KEY, "loop", self.ticked);
        result.insert(AEC_STAT_KEY, "block_conflict", self.conflicts);
        result.insert(AEC_STAT_KEY, "started", self.started);

        for behavior in ElectionBehavior::iter() {
            result.insert(
                "active_elections_started",
                behavior.as_str(),
                self.started_by_behavor[behavior as usize],
            );
        }

        for conf_type in ConfirmationType::iter() {
            result.insert(
                "confirmation_observer",
                conf_type.as_str(),
                self.block_confirmations[conf_type as usize],
            );
        }

        self.stopped_counter.collect_stats(result);
    }
}
