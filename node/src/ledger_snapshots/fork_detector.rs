use crate::{
    block_processing::LedgerEvent, consensus::ActiveElectionsContainer,
    ledger_event_processor::LedgerEventProcessorPlugin, ledger_snapshots::LedgerSnapshots,
};
use rsnano_ledger::{BlockError, Ledger};
use std::sync::{Arc, RwLock};

pub(crate) struct ForkDetector {
    ledger: Arc<Ledger>,
    ledger_snapshots: Arc<LedgerSnapshots>,
    active_election_container: Arc<RwLock<ActiveElectionsContainer>>,
}

impl ForkDetector {
    pub(crate) fn new(
        ledger: Arc<Ledger>,
        ledger_snapshots: Arc<LedgerSnapshots>,
        active_election_container: Arc<RwLock<ActiveElectionsContainer>>,
    ) -> Self {
        Self {
            ledger,
            ledger_snapshots,
            active_election_container,
        }
    }
}

impl LedgerEventProcessorPlugin for ForkDetector {
    fn process(&mut self, event: &LedgerEvent) {
        if let LedgerEvent::BlocksProcessed(results) = event {
            for result in results {
                if result.status == Err(BlockError::Fork) {
                    let root = result.block.qualified_root();
                    tracing::debug!("Fork detected: {:?}", root);

                    self.ledger
                        .mark_fork(&root, self.ledger_snapshots.get_current_snapshot_number());

                    self.active_election_container.write().unwrap().erase(&root);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        block_processing::{BlockSource, LedgerEvent, ProcessedResult},
        consensus::{ActiveElectionsContainer, AecInsertRequest, election::ElectionBehavior},
        ledger_event_processor::LedgerEventProcessorPlugin,
        ledger_snapshots::{LedgerSnapshots, fork_detector::ForkDetector},
    };
    use rsnano_ledger::{BlockError, Ledger};
    use rsnano_nullable_clock::Timestamp;
    use rsnano_types::{Block, BlockPriority, SavedBlock};
    use std::sync::{Arc, RwLock};
    use store_rocksdb::default_ledger_store_factory;

    #[test]
    fn marks_a_forked_block_in_the_ledger() {
        let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));
        let ledger_snapshots = LedgerSnapshots::new_null();
        let active_election_container = ActiveElectionsContainer::default();
        let snapshot_number = ledger_snapshots.get_current_snapshot_number();
        let mut fork_detector = ForkDetector::new(
            ledger.clone(),
            ledger_snapshots.into(),
            RwLock::new(active_election_container).into(),
        );
        let block = Block::new_test_instance();
        let root = block.qualified_root();

        let processed_results = ProcessedResult {
            block,
            source: BlockSource::Live,
            status: Err(BlockError::Fork),
            saved_block: None,
        };

        fork_detector.process(&LedgerEvent::BlocksProcessed(vec![processed_results]));

        assert_eq!(
            ledger.store.forks.get(&ledger.store.begin_read(), &root),
            Some(snapshot_number)
        );
    }

    #[test]
    fn can_mark_multiple_forks_in_one_go() {
        let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));
        let ledger_snapshots = LedgerSnapshots::new_null();
        let active_election_container = ActiveElectionsContainer::default();
        let snapshot_number = ledger_snapshots.get_current_snapshot_number();
        let mut fork_detector = ForkDetector::new(
            ledger.clone(),
            ledger_snapshots.into(),
            RwLock::new(active_election_container).into(),
        );
        let block1 = Block::new_test_instance_with_key(1);
        let block2 = Block::new_test_instance_with_key(2);
        let root1 = block1.qualified_root();
        let root2 = block2.qualified_root();

        let processed_result1 = ProcessedResult {
            block: block1,
            source: BlockSource::Live,
            status: Err(BlockError::Fork),
            saved_block: None,
        };

        let processed_result2 = ProcessedResult {
            block: block2,
            source: BlockSource::Live,
            status: Err(BlockError::Fork),
            saved_block: None,
        };

        fork_detector.process(&LedgerEvent::BlocksProcessed(vec![
            processed_result1,
            processed_result2,
        ]));

        assert_eq!(
            ledger.store.forks.get(&ledger.store.begin_read(), &root1),
            Some(snapshot_number)
        );

        assert_eq!(
            ledger.store.forks.get(&ledger.store.begin_read(), &root2),
            Some(snapshot_number)
        );
    }

    #[test]
    fn ignores_blocks_without_fork() {
        let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));
        let ledger_snapshots = LedgerSnapshots::new_null();
        let active_election_container = ActiveElectionsContainer::default();
        let mut fork_detector = ForkDetector::new(
            ledger.clone(),
            ledger_snapshots.into(),
            RwLock::new(active_election_container).into(),
        );
        let block = Block::new_test_instance();
        let root = block.qualified_root();

        let processed_results = ProcessedResult {
            block,
            source: BlockSource::Live,
            status: Err(BlockError::GapPrevious),
            saved_block: None,
        };

        fork_detector.process(&LedgerEvent::BlocksProcessed(vec![processed_results]));

        assert_eq!(
            ledger.store.forks.get(&ledger.store.begin_read(), &root),
            None
        );
    }

    #[test]
    fn stop_forked_election() {
        let block = SavedBlock::new_test_instance();
        let mut container = ActiveElectionsContainer::default();
        let request = AecInsertRequest {
            block: block.clone(),
            behavior: ElectionBehavior::Priority,
            priority: BlockPriority::new_test_instance(),
        };

        container
            .insert(request, Timestamp::new_test_instance())
            .unwrap();

        let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));
        let ledger_snapshots = LedgerSnapshots::new_null();
        let mut fork_detector = ForkDetector::new(
            ledger.clone(),
            ledger_snapshots.into(),
            RwLock::new(container).into(),
        );

        let processed_results = ProcessedResult {
            block: block.into(),
            source: BlockSource::Live,
            status: Err(BlockError::Fork),
            saved_block: None,
        };

        fork_detector.process(&LedgerEvent::BlocksProcessed(vec![processed_results]));

        assert_eq!(
            fork_detector
                .active_election_container
                .read()
                .unwrap()
                .len(),
            0
        );
    }
}
