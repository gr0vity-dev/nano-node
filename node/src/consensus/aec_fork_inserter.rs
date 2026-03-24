use std::sync::{Arc, Mutex, RwLock};

use tracing::debug;

use crate::block_processing::LedgerPipelineEvent;
use rsnano_ledger::{BlockError, LedgerEvent, ProcessResult, RepWeightCache};
use rsnano_types::{Amount, Block, BlockHash, QualifiedRoot};
use rsnano_utils::{EventHandlerMut, stats::Stats};

use super::{ActiveElectionsContainer, AecEventPublisher, ForkCache, VoteCache};

pub(crate) struct AecForkInserter {
    pub(crate) rep_weights: Arc<RepWeightCache>,
    pub(crate) fork_cache: Arc<RwLock<ForkCache>>,
    pub(crate) active_elections: Arc<RwLock<ActiveElectionsContainer>>,
    pub(crate) vote_cache: Arc<Mutex<VoteCache>>,
    pub(crate) publisher: AecEventPublisher,
}

impl AecForkInserter {
    #[allow(dead_code)]
    pub fn new_test_instance() -> Self {
        Self {
            rep_weights: Arc::new(RepWeightCache::default()),
            fork_cache: Arc::new(RwLock::new(ForkCache::new())),
            active_elections: Arc::new(RwLock::new(ActiveElectionsContainer::default())),
            vote_cache: Arc::new(Mutex::new(VoteCache::new(
                Default::default(),
                Arc::new(Stats::default()),
            ))),
            publisher: AecEventPublisher::null(),
        }
    }

    pub fn handle_forks(&self, batch: &[ProcessResult]) {
        for result in batch {
            if result.status == Err(BlockError::Fork) {
                self.handle_fork(&result.block);
            }
        }
    }

    pub fn try_add_cached_forks(&self, root: &QualifiedRoot) {
        let fork_cache = self.fork_cache.read().unwrap();
        for fork in fork_cache.get_forks(root) {
            self.handle_fork(fork);
        }
    }

    fn handle_fork(&self, fork: &Block) {
        let fork_tally = self.get_cached_tally(&fork.hash());

        let result = self
            .active_elections
            .write()
            .unwrap()
            .try_add_fork(fork, fork_tally);

        self.publisher.publish_all(result.events);

        if result.value {
            debug!("Block was added to an existing election: {}", fork.hash());
        }
    }

    fn get_cached_tally(&self, hash: &BlockHash) -> Amount {
        let votes = self.vote_cache.lock().unwrap().find(hash);
        let mut tally = Amount::ZERO;
        let weights = self.rep_weights.read();
        for vote in votes {
            tally += weights.weight(&vote.voter);
        }
        tally
    }
}

pub(crate) struct ForkInserterPlugin {
    fork_processor: Arc<AecForkInserter>,
}

impl ForkInserterPlugin {
    pub fn new(fork_processor: Arc<AecForkInserter>) -> Self {
        Self { fork_processor }
    }
}

impl EventHandlerMut<LedgerPipelineEvent> for ForkInserterPlugin {
    fn handle(&mut self, event: &LedgerPipelineEvent) {
        if let LedgerPipelineEvent::Ledger(LedgerEvent::BlocksProcessed(results)) = event {
            // Notify elections about alternative (forked) blocks
            self.fork_processor.handle_forks(results);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::AecEvent;
    use crate::consensus::AecInsertRequest;
    use rsnano_nullable_clock::Timestamp;
    use rsnano_types::{BlockPriority, SavedBlock, StateBlockArgs};
    use rsnano_utils::sync::backpressure_channel::channel;

    #[test]
    fn publishes_returned_fork_events() {
        let (aec_sender, aec_receiver) = channel(8);
        let active_elections = Arc::new(RwLock::new(ActiveElectionsContainer::default()));
        let winner_args = StateBlockArgs::new_test_instance();
        let winner = SavedBlock::new_test_instance_with(winner_args.clone().into());
        let fork = Block::from(StateBlockArgs {
            representative: 42.into(),
            ..winner_args
        });
        let fork_hash = fork.hash();

        active_elections
            .write()
            .unwrap()
            .insert(
                AecInsertRequest::new_priority(winner, BlockPriority::new_test_instance()),
                Timestamp::new_test_instance(),
            )
            .unwrap();

        let inserter = AecForkInserter {
            rep_weights: Arc::new(RepWeightCache::default()),
            fork_cache: Arc::new(RwLock::new(ForkCache::new())),
            active_elections,
            vote_cache: Arc::new(Mutex::new(VoteCache::new(
                Default::default(),
                Arc::new(Stats::default()),
            ))),
            publisher: AecEventPublisher::new(aec_sender),
        };

        inserter.handle_fork(&fork);

        assert!(matches!(
            aec_receiver.try_recv(),
            Ok(AecEvent::BlockAddedToElection(hash)) if hash == fork_hash
        ));
    }
}
