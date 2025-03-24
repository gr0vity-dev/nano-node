use strum::EnumCount;
use strum_macros::EnumCount as EnumCountMacro;

#[derive(Clone, Debug, PartialEq, EnumCountMacro, num_derive::FromPrimitive)]
pub enum AecCooldownReason {
    ConfirmingSetFull,
    ConfirmingSetEventQueueFull,
    AecEventQueueFull,
}

pub(crate) struct CooldownController {
    // Array of cooldown states, indexed by CooldownSource as usize
    source_states: [bool; AecCooldownReason::COUNT],
    cool_down: bool,
}

impl CooldownController {
    pub fn new() -> Self {
        Self {
            source_states: [false; AecCooldownReason::COUNT],
            cool_down: false,
        }
    }

    pub fn is_cooling_down(&self) -> bool {
        self.cool_down
    }

    pub fn is_source_cooling_down(&self, source: AecCooldownReason) -> bool {
        self.source_states[source as usize]
    }

    pub fn reason(&self) -> Option<AecCooldownReason> {
        for i in 0..AecCooldownReason::COUNT {
            if self.source_states[i] {
                // Convert index back to enum variant
                return num_traits::FromPrimitive::from_usize(i);
            }
        }
        None
    }

    pub fn set_cooldown(&mut self, cooldown_source: AecCooldownReason, cool_down: bool) {
        // Update the specific source state
        let index = cooldown_source as usize;
        self.source_states[index] = cool_down;

        // Update overall cooldown state - true if any source is cooling down
        self.cool_down = self.source_states.iter().any(|&state| state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_cooldown() {
        let controller = CooldownController::new();
        assert_eq!(controller.is_cooling_down(), false);
    }

    #[test]
    fn one_cooldown() {
        let mut controller = CooldownController::new();
        controller.set_cooldown(AecCooldownReason::ConfirmingSetFull, true);
        assert_eq!(controller.is_cooling_down(), true);
    }

    #[test]
    fn end_cooldown() {
        let mut controller = CooldownController::new();
        controller.set_cooldown(AecCooldownReason::ConfirmingSetFull, true);
        controller.set_cooldown(AecCooldownReason::ConfirmingSetFull, false);
        assert_eq!(controller.is_cooling_down(), false);
    }

    #[test]
    fn mutliple_sources_different_cooldowns() {
        let mut controller = CooldownController::new();
        controller.set_cooldown(AecCooldownReason::ConfirmingSetFull, true);
        controller.set_cooldown(AecCooldownReason::AecEventQueueFull, false);
        assert_eq!(controller.is_cooling_down(), true);
    }
}
