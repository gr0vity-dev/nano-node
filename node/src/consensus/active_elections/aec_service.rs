use std::{
    collections::HashMap,
    sync::{LockResult, RwLock, RwLockReadGuard, RwLockWriteGuard},
    time::Duration,
};

use rsnano_nullable_clock::SteadyClock;
use rsnano_types::{Account, Amount, Block, BlockHash, QualifiedRoot, Root, VoteError};
use rsnano_utils::sync::backpressure_channel::Sender;

use crate::consensus::{AecCooldownReason, ApplyVoteArgs};

use super::{
    ActiveElectionsConfig, ActiveElectionsContainer, AecFact, AecInsertError, AecInsertRequest,
};
use crate::consensus::election::{ConfirmedElection, Election, ElectionState, VoteType};

pub struct AecService {
    active: RwLock<ActiveElectionsContainer>,
    clock: std::sync::Arc<SteadyClock>,
}

impl AecService {
    pub fn new(
        config: ActiveElectionsConfig,
        base_latency: Duration,
        clock: std::sync::Arc<SteadyClock>,
    ) -> Self {
        Self {
            active: RwLock::new(ActiveElectionsContainer::new(config, base_latency)),
            clock,
        }
    }

    pub fn new_null() -> Self {
        Self::new(
            ActiveElectionsConfig::default(),
            Duration::from_secs(1),
            std::sync::Arc::new(SteadyClock::new_null()),
        )
    }

    pub fn read(&self) -> LockResult<RwLockReadGuard<'_, ActiveElectionsContainer>> {
        self.active.read()
    }

    pub fn write(&self) -> LockResult<RwLockWriteGuard<'_, ActiveElectionsContainer>> {
        self.active.write()
    }

    pub fn now(&self) -> rsnano_nullable_clock::Timestamp {
        self.clock.now()
    }

    pub fn set_observer(&self, observer: Sender<AecFact>) {
        self.active.write().unwrap().set_observer(observer);
    }

    pub fn insert(&self, request: AecInsertRequest) -> Result<(), AecInsertError> {
        self.active
            .write()
            .unwrap()
            .insert(request, self.clock.now())
    }

    pub fn apply_vote(&self, args: ApplyVoteArgs<'_>) -> HashMap<BlockHash, Result<(), VoteError>> {
        self.active.write().unwrap().apply_vote(args)
    }

    pub fn confirm_dependent_elections(
        &self,
        confirmed: Vec<(rsnano_types::SavedBlock, Option<ConfirmedElection>)>,
    ) {
        self.active
            .write()
            .unwrap()
            .confirm_dependent_elections(confirmed, self.clock.now());
    }

    pub fn try_add_fork(&self, fork: &Block, fork_tally: Amount) -> bool {
        self.active.write().unwrap().try_add_fork(fork, fork_tally)
    }

    pub fn transition_time(&self) {
        self.active
            .write()
            .unwrap()
            .transition_time(self.clock.now());
    }

    pub fn transition_active(&self, block_hash: &BlockHash) -> bool {
        self.active.write().unwrap().transition_active(block_hash)
    }

    pub fn election(&self, root: &QualifiedRoot) -> Option<Election> {
        self.active.read().unwrap().election_for_root(root).cloned()
    }

    pub fn election_for_block(&self, block_hash: &BlockHash) -> Option<Election> {
        self.active
            .read()
            .unwrap()
            .election_for_block(block_hash)
            .cloned()
    }

    pub fn stale_election_accounts(
        &self,
        now: rsnano_nullable_clock::Timestamp,
        stale_threshold: Duration,
        max_results: usize,
    ) -> Vec<Account> {
        self.active
            .read()
            .unwrap()
            .iter_round_robin()
            .filter(|election| election.start().elapsed(now) >= stale_threshold)
            .map(|election| election.account())
            .take(max_results)
            .collect()
    }

    pub fn active_elections(&self) -> Vec<Election> {
        self.active
            .read()
            .unwrap()
            .iter_round_robin()
            .filter(|election| election.state() == ElectionState::Active)
            .cloned()
            .collect()
    }

    pub fn confirmation_active_roots(&self, announcements: u64) -> (Vec<QualifiedRoot>, u64) {
        let mut confirmed = 0;
        let confirmations = self
            .active
            .read()
            .unwrap()
            .iter_round_robin()
            .filter_map(|election| {
                let req_count = 0_u64; // not supported in RsNano
                if req_count < announcements {
                    return None;
                }

                if election.is_confirmed() {
                    confirmed += 1;
                    None
                } else {
                    Some(election.qualified_root().clone())
                }
            })
            .collect();

        (confirmations, confirmed)
    }

    pub fn remove_votes(
        &self,
        root: &QualifiedRoot,
        voters: impl IntoIterator<Item = rsnano_types::PublicKey>,
    ) {
        let voters = voters.into_iter().collect::<Vec<_>>();
        self.active
            .write()
            .unwrap()
            .remove_votes(root, voters.iter());
    }

    pub fn erase(&self, root: &QualifiedRoot) -> bool {
        self.active.write().unwrap().erase(root)
    }

    pub fn remove_recently_confirmed(&self, block_hash: &BlockHash) {
        self.active
            .write()
            .unwrap()
            .remove_recently_confirmed(block_hash);
    }

    pub fn set_cooldown(&self, cool_down: bool, reason: AecCooldownReason) {
        self.active.write().unwrap().set_cooldown(cool_down, reason);
    }

    pub fn force_confirm(&self, block_hash: &BlockHash) {
        self.active
            .write()
            .unwrap()
            .force_confirm(block_hash, self.clock.now());
    }

    pub fn next_vote_to_broadcast(
        &self,
        bucket_id: usize,
        vote_broadcast_interval: Duration,
    ) -> Option<(Root, BlockHash, VoteType)> {
        let now = self.clock.now();
        let mut active = self.active.write().unwrap();
        let vote_target = active.iter_bucket(bucket_id).find_map(|election| {
            if election.can_vote(vote_broadcast_interval, now) {
                Some((
                    election.qualified_root().clone(),
                    election.vote_type(),
                    election.winner().hash(),
                ))
            } else {
                None
            }
        });

        vote_target.map(|(qualified_root, vote_type, winner_hash)| {
            active.set_last_voted(&qualified_root, vote_type, now);
            (qualified_root.root, winner_hash, vote_type)
        })
    }

    pub fn stop(&self) {
        self.active.write().unwrap().stop();
    }
}

impl rsnano_utils::stats::StatsSource for AecService {
    fn collect_stats(&self, result: &mut rsnano_utils::stats::StatsCollection) {
        self.active.read().unwrap().collect_stats(result);
    }
}

impl rsnano_utils::container_info::ContainerInfoProvider for AecService {
    fn container_info(&self) -> rsnano_utils::container_info::ContainerInfo {
        self.active.read().unwrap().container_info()
    }
}
