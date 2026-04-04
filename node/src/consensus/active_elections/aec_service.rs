use std::{collections::HashMap, sync::RwLock, time::Duration};

use rsnano_nullable_clock::Timestamp;
use rsnano_types::{Amount, Block, BlockHash, PublicKey, QualifiedRoot, SavedBlock, VoteError};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{StatsCollection, StatsSource},
    sync::backpressure_channel::Sender,
};

use super::{
    ActiveElectionsConfig, ActiveElectionsContainer, ActiveElectionsInfo, AecCooldownReason,
    AecFact, AecInsertError, AecInsertRequest, ApplyVoteArgs,
};
use crate::consensus::{
    election::{ConfirmedElection, Election, ElectionBehavior},
    ElectionCandidateSource,
};

pub struct AecService {
    aec: RwLock<ActiveElectionsContainer>,
    publisher: RwLock<Option<Sender<AecFact>>>,
}

impl AecService {
    pub fn new(config: ActiveElectionsConfig, base_latency: Duration) -> Self {
        Self {
            aec: RwLock::new(ActiveElectionsContainer::new(config, base_latency)),
            publisher: RwLock::new(None),
        }
    }

    pub fn new_null() -> Self {
        Self {
            aec: RwLock::new(ActiveElectionsContainer::default()),
            publisher: RwLock::new(None),
        }
    }

    // --- Read forwarding ---

    pub fn check_vacancy<T>(&self, source: &T) -> bool
    where
        T: ElectionCandidateSource,
    {
        self.aec.read().unwrap().check_vacancy(source)
    }

    pub fn election_for_root(&self, root: &QualifiedRoot) -> Option<Election> {
        self.aec.read().unwrap().election_for_root(root).cloned()
    }

    pub fn election_for_block(&self, block_hash: &BlockHash) -> Option<Election> {
        self.aec
            .read()
            .unwrap()
            .election_for_block(block_hash)
            .cloned()
    }

    pub fn max_len(&self) -> usize {
        self.aec.read().unwrap().max_len()
    }

    pub fn len(&self) -> usize {
        self.aec.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.aec.read().unwrap().is_empty()
    }

    pub fn is_active_root(&self, root: &QualifiedRoot) -> bool {
        self.aec.read().unwrap().is_active_root(root)
    }

    pub fn is_active_hash(&self, block_hash: &BlockHash) -> bool {
        self.aec.read().unwrap().is_active_hash(block_hash)
    }

    pub fn was_recently_confirmed(&self, block_hash: &BlockHash) -> bool {
        self.aec.read().unwrap().was_recently_confirmed(block_hash)
    }

    pub fn count_by_behavior(&self, behavior: ElectionBehavior) -> usize {
        self.aec.read().unwrap().count_by_behavior(behavior)
    }

    pub fn vacancy(&self) -> i64 {
        self.aec.read().unwrap().vacancy()
    }

    pub fn info(&self) -> ActiveElectionsInfo {
        self.aec.read().unwrap().info()
    }

    pub fn round_robin<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut dyn Iterator<Item = &Election>) -> T,
    {
        let guard = self.aec.read().unwrap();
        f(&mut guard.iter_round_robin())
    }

    pub fn pick_one_election_per_bucket<P, F, T>(
        &self,
        starting_bucket: usize,
        filter: F,
        process: P,
    ) -> T
    where
        F: Fn(&Election) -> bool,
        P: FnOnce(&mut dyn Iterator<Item = (usize, &Election)>) -> T,
    {
        let guard = self.aec.read().unwrap();
        process(&mut guard.pick_one_per_bucket_from(starting_bucket, filter))
    }

    // --- Write forwarding ---

    pub fn set_observer(&self, observer: Sender<AecFact>) {
        *self.publisher.write().unwrap() = Some(observer);
    }

    pub fn insert(&self, request: AecInsertRequest, now: Timestamp) -> Result<(), AecInsertError> {
        let produced_facts = self.aec.write().unwrap().insert(request, now)?;
        self.publish(produced_facts);
        Ok(())
    }

    pub fn try_add_fork(&self, fork: &Block, fork_tally: Amount) -> bool {
        let (added, produced_facts) = self.aec.write().unwrap().try_add_fork(fork, fork_tally);
        self.publish(produced_facts);
        added
    }

    pub fn apply_vote<'a>(
        &self,
        args: ApplyVoteArgs<'a>,
    ) -> HashMap<BlockHash, Result<(), VoteError>> {
        let (results, produced_facts) = self.aec.write().unwrap().apply_vote(args);
        self.publish(produced_facts);
        results
    }

    pub fn transition_time(&self, now: Timestamp) {
        let produced_facts = self.aec.write().unwrap().transition_time(now);
        self.publish(produced_facts);
    }

    pub fn transition_active(&self, block_hash: &BlockHash) -> bool {
        self.aec.write().unwrap().transition_active(block_hash)
    }

    pub fn refill<T>(&self, source: &mut T, now: Timestamp)
    where
        T: ElectionCandidateSource,
    {
        let produced_facts = self.aec.write().unwrap().refill(source, now);
        self.publish(produced_facts);
    }

    pub fn remove_votes<'a>(
        &self,
        root: &QualifiedRoot,
        voters: impl IntoIterator<Item = &'a PublicKey>,
    ) {
        self.aec.write().unwrap().remove_votes(root, voters)
    }

    pub fn erase(&self, root: &QualifiedRoot) -> bool {
        let Some(produced_facts) = self.aec.write().unwrap().erase(root) else {
            return false;
        };
        self.publish(produced_facts);
        true
    }

    pub fn confirm_dependent_elections(
        &self,
        confirmed: Vec<(SavedBlock, Option<ConfirmedElection>)>,
        now: Timestamp,
    ) {
        let produced_facts = self
            .aec
            .write()
            .unwrap()
            .confirm_dependent_elections(confirmed, now);
        self.publish(produced_facts);
    }

    pub fn remove_recently_confirmed(&self, block_hash: &BlockHash) {
        self.aec
            .write()
            .unwrap()
            .remove_recently_confirmed(block_hash)
    }

    pub fn set_cooldown(&self, cool_down: bool, reason: AecCooldownReason) {
        let produced_facts = self.aec.write().unwrap().set_cooldown(cool_down, reason);
        self.publish(produced_facts);
    }

    pub fn cancel(&self, root: &QualifiedRoot) {
        self.aec.write().unwrap().cancel(root)
    }

    pub fn cancel_all(&self) {
        self.aec.write().unwrap().cancel_all()
    }

    pub fn clear_recently_confirmed(&self) {
        self.aec.write().unwrap().clear_recently_confirmed()
    }

    pub fn stop(&self) {
        drop(self.publisher.write().unwrap().take());
        self.aec.write().unwrap().stop()
    }

    pub fn force_confirm(&self, block_hash: &BlockHash, now: Timestamp) {
        let produced_facts = self.aec.write().unwrap().force_confirm(block_hash, now);
        self.publish(produced_facts);
    }

    pub fn simulate_event(&self, event: AecFact) {
        let produced_facts = self.aec.read().unwrap().simulate_event(event);
        self.publish(produced_facts);
    }

    fn publish(&self, produced_facts: super::ProducedAecFacts) {
        if produced_facts.is_empty() {
            return;
        }

        if let Some(sender) = self.publisher.read().unwrap().as_ref() {
            for fact in produced_facts {
                sender.send(fact).unwrap();
            }
        }
    }
}

impl StatsSource for AecService {
    fn collect_stats(&self, result: &mut StatsCollection) {
        self.aec.read().unwrap().collect_stats(result)
    }
}

impl ContainerInfoProvider for AecService {
    fn container_info(&self) -> ContainerInfo {
        self.aec.read().unwrap().container_info()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::{BucketInfo, ElectionCandidate, election::ElectionBehavior};
    use rsnano_nullable_clock::Timestamp;
    use rsnano_types::{BlockPriority, SavedBlock};
    use rsnano_utils::sync::backpressure_channel;

    #[test]
    fn insert_publishes_container_facts_through_service_owned_sender() {
        let service = AecService::new_null();
        let (tx, rx) = backpressure_channel::channel(1);
        service.set_observer(tx);

        service
            .insert(
                AecInsertRequest {
                    block: SavedBlock::new_test_instance(),
                    behavior: ElectionBehavior::Priority,
                    priority: BlockPriority::new_test_instance(),
                },
                Timestamp::new_test_instance(),
            )
            .unwrap();

        assert!(matches!(rx.try_recv(), Ok(AecFact::ElectionStarted(_, _))));
    }

    #[test]
    fn simulate_event_uses_service_owned_publication_path() {
        let service = AecService::new_null();
        let (tx, rx) = backpressure_channel::channel(1);
        service.set_observer(tx);

        service.simulate_event(AecFact::Recovered);

        assert!(matches!(rx.try_recv(), Ok(AecFact::Recovered)));
    }

    #[test]
    fn refill_publishes_scheduler_driven_activation_through_service_owned_sender() {
        let service = AecService::new_null();
        let (tx, rx) = backpressure_channel::channel(1);
        service.set_observer(tx);

        let block = SavedBlock::new_test_instance();
        let mut source = StubCandidateSource::new(block.clone(), BlockPriority::new_test_instance());
        service.refill(&mut source, Timestamp::new_test_instance());

        assert!(matches!(
            rx.try_recv(),
            Ok(AecFact::ElectionStarted(hash, root)) if hash == block.hash() && root == block.qualified_root()
        ));
    }

    struct StubCandidateSource {
        candidates: Vec<ElectionCandidate>,
    }

    impl StubCandidateSource {
        fn new(block: SavedBlock, priority: BlockPriority) -> Self {
            Self {
                candidates: vec![ElectionCandidate {
                    bucket_id: 0,
                    block,
                    priority,
                }],
            }
        }
    }

    impl ElectionCandidateSource for StubCandidateSource {
        fn should_schedule(&self, _buckets: &[BucketInfo]) -> bool {
            !self.candidates.is_empty()
        }

        fn gather_candidates(
            &mut self,
            _buckets: &[BucketInfo],
            result: &mut Vec<ElectionCandidate>,
        ) {
            result.append(&mut self.candidates);
        }
    }
}
