use std::sync::{Arc, RwLock};

use super::{ActiveElectionsContainer, AecEventPublisher, election::ConfirmedElection};
use crate::cementation::ConfirmingSet;
use rsnano_nullable_clock::SteadyClock;
use rsnano_types::{BlockHash, SavedBlock};

pub(crate) struct DependentElectionsConfirmer {
    pub(crate) confirming_set: Arc<ConfirmingSet>,
    pub(crate) active_elections: Arc<RwLock<ActiveElectionsContainer>>,
    pub(crate) clock: Arc<SteadyClock>,
    pub(crate) publisher: AecEventPublisher,
}

impl DependentElectionsConfirmer {
    pub fn new_null() -> Self {
        Self {
            confirming_set: Arc::new(ConfirmingSet::new_null()),
            active_elections: Arc::new(RwLock::new(ActiveElectionsContainer::default())),
            clock: Arc::new(SteadyClock::new_null()),
            publisher: AecEventPublisher::null(),
        }
    }

    /// Confirmed blocks might implicitly confirm dependent elections
    pub fn confirm_dependent_elections(&self, confirmed_blocks: &Vec<(SavedBlock, BlockHash)>) {
        let blocks_plus_election = self.blocks_plus_elections(confirmed_blocks);
        let now = self.clock.now();

        let result = self
            .active_elections
            .write()
            .unwrap()
            .confirm_dependent_elections(blocks_plus_election, now);

        self.publisher.publish_all(result.events);
    }

    fn blocks_plus_elections(
        &self,
        blocks: &Vec<(SavedBlock, BlockHash)>,
    ) -> Vec<(SavedBlock, Option<ConfirmedElection>)> {
        let mut blocks_with_election = Vec::with_capacity(blocks.len());

        self.confirming_set.do_election_cache(|cache| {
            for (confirmed_block, _) in blocks {
                let source_election = cache.get(&confirmed_block.hash()).cloned();
                blocks_with_election.push((confirmed_block.clone(), source_election));
            }
        });

        blocks_with_election
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::AecEvent;
    use crate::consensus::AecInsertRequest;
    use rsnano_nullable_clock::Timestamp;
    use rsnano_types::{BlockPriority, SavedBlock};
    use rsnano_utils::sync::backpressure_channel::channel;

    #[test]
    fn publishes_returned_block_confirmed_events() {
        let (aec_sender, aec_receiver) = channel(8);
        let active_elections = Arc::new(RwLock::new(ActiveElectionsContainer::default()));
        let block = SavedBlock::new_test_instance();
        let hash = block.hash();

        active_elections
            .write()
            .unwrap()
            .insert(
                AecInsertRequest::new_priority(block.clone(), BlockPriority::new_test_instance()),
                Timestamp::new_test_instance(),
            )
            .unwrap();

        let confirmer = DependentElectionsConfirmer {
            confirming_set: Arc::new(ConfirmingSet::new_null()),
            active_elections,
            clock: Arc::new(SteadyClock::new_null()),
            publisher: AecEventPublisher::new(aec_sender),
        };

        confirmer.confirm_dependent_elections(&vec![(block, hash)]);

        assert!(matches!(
            aec_receiver.try_recv(),
            Ok(AecEvent::BlockConfirmed(confirmed_block, _)) if confirmed_block.hash() == hash
        ));
    }
}
