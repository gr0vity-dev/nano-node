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
}

impl AecService {
    pub fn new(config: ActiveElectionsConfig, base_latency: Duration) -> Self {
        Self {
            aec: RwLock::new(ActiveElectionsContainer::new(config, base_latency)),
        }
    }

    pub fn new_null() -> Self {
        Self {
            aec: RwLock::new(ActiveElectionsContainer::default()),
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

    pub fn with_elections_starting_from_bucket<F, T>(&self, starting_bucket: usize, f: F) -> T
    where
        F: FnOnce(&mut dyn Iterator<Item = (usize, &Election)>) -> T,
    {
        let guard = self.aec.read().unwrap();
        f(&mut guard.iter_from_bucket(starting_bucket))
    }

    // --- Write forwarding ---

    pub fn set_observer(&self, observer: Sender<AecFact>) {
        self.aec.write().unwrap().set_observer(observer)
    }

    pub fn insert(&self, request: AecInsertRequest, now: Timestamp) -> Result<(), AecInsertError> {
        self.aec.write().unwrap().insert(request, now)
    }

    pub fn try_add_fork(&self, fork: &Block, fork_tally: Amount) -> bool {
        self.aec.write().unwrap().try_add_fork(fork, fork_tally)
    }

    pub fn apply_vote<'a>(
        &self,
        args: ApplyVoteArgs<'a>,
    ) -> HashMap<BlockHash, Result<(), VoteError>> {
        self.aec.write().unwrap().apply_vote(args)
    }

    pub fn transition_time(&self, now: Timestamp) {
        self.aec.write().unwrap().transition_time(now)
    }

    pub fn transition_active(&self, block_hash: &BlockHash) -> bool {
        self.aec.write().unwrap().transition_active(block_hash)
    }

    pub fn refill<T>(&self, source: &mut T, now: Timestamp)
    where
        T: ElectionCandidateSource,
    {
        self.aec.write().unwrap().refill(source, now);
    }

    pub fn remove_votes<'a>(
        &self,
        root: &QualifiedRoot,
        voters: impl IntoIterator<Item = &'a PublicKey>,
    ) {
        self.aec.write().unwrap().remove_votes(root, voters)
    }

    pub fn erase(&self, root: &QualifiedRoot) -> bool {
        self.aec.write().unwrap().erase(root)
    }

    pub fn confirm_dependent_elections(
        &self,
        confirmed: Vec<(SavedBlock, Option<ConfirmedElection>)>,
        now: Timestamp,
    ) {
        self.aec
            .write()
            .unwrap()
            .confirm_dependent_elections(confirmed, now)
    }

    pub fn remove_recently_confirmed(&self, block_hash: &BlockHash) {
        self.aec
            .write()
            .unwrap()
            .remove_recently_confirmed(block_hash)
    }

    pub fn set_cooldown(&self, cool_down: bool, reason: AecCooldownReason) {
        self.aec.write().unwrap().set_cooldown(cool_down, reason)
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
        self.aec.write().unwrap().stop()
    }

    pub fn force_confirm(&self, block_hash: &BlockHash, now: Timestamp) {
        self.aec.write().unwrap().force_confirm(block_hash, now)
    }

    pub fn simulate_event(&self, event: AecFact) {
        self.aec.read().unwrap().simulate_event(event)
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
