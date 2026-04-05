use std::sync::Arc;

use rsnano_ledger::LedgerEvent;
use rsnano_utils::EventHandlerMut;

use crate::block_processing::LedgerPipelineEvent;

use super::ElectionSchedulers;

pub(crate) struct ElectionSchedulersPlugin {
    schedulers: Arc<ElectionSchedulers>,
}

impl ElectionSchedulersPlugin {
    pub(crate) fn new(schedulers: Arc<ElectionSchedulers>) -> Self {
        Self { schedulers }
    }
}

impl EventHandlerMut<LedgerPipelineEvent> for ElectionSchedulersPlugin {
    fn handle(&mut self, event: &LedgerPipelineEvent) {
        if let LedgerPipelineEvent::Ledger(event) = event {
            match event {
                LedgerEvent::BlocksProcessed(results) => {
                    self.schedulers.activate_accounts_with_fresh_blocks(results);
                }
                LedgerEvent::BlocksConfirmed(confirmed) => {
                    self.schedulers
                        .activate_successors(confirmed.iter().map(|(b, _)| b));
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_types::{BlockHash, SavedBlock};

    #[test]
    fn when_blocks_confirmed_should_activate_elections_for_sucessors() {
        let schedulers = Arc::new(ElectionSchedulers::new_null());
        let mut processor = ElectionSchedulersPlugin::new(schedulers.clone());
        let activation_tracker = schedulers.priority.track_activate_successors();

        let block = SavedBlock::new_test_instance();
        let confirmed_blocks = vec![(block.clone(), BlockHash::from(123))];
        processor.handle(&LedgerPipelineEvent::Ledger(LedgerEvent::BlocksConfirmed(
            confirmed_blocks,
        )));

        let output = activation_tracker.output();
        assert_eq!(output, [block]);
    }
}
