use super::AEC_STAT_KEY;
use rsnano_utils::stats::{StatsCollection, StatsSource};
use strum::EnumCount;
use strum_macros::EnumCount as EnumCountMacro;

#[derive(Clone, Debug, PartialEq, EnumCountMacro)]
pub enum AecCooldownReason {
    ConfirmingSetFull,
    AecFactQueueFull,
}

#[derive(Default)]
pub(crate) struct CooldownController {
    // Array of cooldown states, indexed by CooldownSource as usize
    source_states: [bool; AecCooldownReason::COUNT],
    cool_down: bool,
    cooldown_count: u64,
    recover_count: u64,
}

impl CooldownController {
    pub fn is_cooling_down(&self) -> bool {
        self.cool_down
    }

    pub fn set_cooldown(&mut self, cool_down: bool, reason: AecCooldownReason) -> CooldownResult {
        let was_cooling_down_before = self.is_cooling_down();

        // Update the specific source state
        let index = reason as usize;
        self.source_states[index] = cool_down;

        // Update overall cooldown state - true if any source is cooling down
        self.cool_down = self.source_states.iter().any(|&state| state);

        if self.is_cooling_down() && !was_cooling_down_before {
            self.cooldown_count += 1;
            CooldownResult::CooldownStarted
        } else if !self.is_cooling_down() && was_cooling_down_before {
            self.recover_count += 1;
            CooldownResult::Recovered
        } else {
            CooldownResult::Unchanged
        }
    }
}

#[derive(PartialEq, Eq, Debug)]
pub(crate) enum CooldownResult {
    CooldownStarted,
    Recovered,
    Unchanged,
}

impl StatsSource for CooldownController {
    fn collect_stats(&self, result: &mut StatsCollection) {
        result.insert(AEC_STAT_KEY, "cooldown", self.cooldown_count);
        result.insert(AEC_STAT_KEY, "recovered", self.recover_count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_cooldown() {
        let controller = CooldownController::default();
        assert_eq!(controller.is_cooling_down(), false);
    }

    #[test]
    fn one_cooldown() {
        let mut controller = CooldownController::default();
        let result = controller.set_cooldown(true, AecCooldownReason::ConfirmingSetFull);
        assert_eq!(result, CooldownResult::CooldownStarted);
        assert_eq!(controller.is_cooling_down(), true);
    }

    #[test]
    fn end_cooldown() {
        let mut controller = CooldownController::default();
        controller.set_cooldown(true, AecCooldownReason::ConfirmingSetFull);
        let result = controller.set_cooldown(false, AecCooldownReason::ConfirmingSetFull);
        assert_eq!(result, CooldownResult::Recovered);
        assert_eq!(controller.is_cooling_down(), false);
    }

    #[test]
    fn mutliple_sources_different_cooldowns() {
        let mut controller = CooldownController::default();
        controller.set_cooldown(true, AecCooldownReason::ConfirmingSetFull);
        let result = controller.set_cooldown(false, AecCooldownReason::AecFactQueueFull);
        assert_eq!(result, CooldownResult::Unchanged);
        assert_eq!(controller.is_cooling_down(), true);
    }
}
