use std::{
    collections::{HashMap, HashSet},
    sync::RwLock,
    time::Duration,
};

use rsnano_nullable_clock::Timestamp;
use rsnano_types::{
    Account, Amount, Block, BlockHash, PublicKey, QualifiedRoot, SavedBlock, TimePriority,
    VoteError,
};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{StatsCollection, StatsSource},
    sync::backpressure_channel::Sender,
};
use strum::EnumCount;

use super::{
    ActiveElectionsConfig, ActiveElectionsContainer, ActiveElectionsInfo, AecCooldownReason,
    AecFact, AecInsertError, AecInsertRequest, ApplyVoteArgs,
    apply_vote_helper::ApplyVoteHelper,
    RootContainer,
    cooldown_controller::{CooldownController, CooldownResult},
    recently_confirmed_cache::RecentlyConfirmedCache,
    root_container::{BucketCursor, ElectionHandle},
    stats::AecStats,
    active_elections_container::{ForkChange, InsertResult},
};
use crate::consensus::election::{
    AddForkResult, ConfirmationType, ConfirmedElection, Election, ElectionBehavior, ElectionState,
    VoteType,
};
use crate::consensus::election_schedulers::priority::bucket_count;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriorityActivationResult {
    Activated,
    ActivatedWithReplacement,
    Duplicate,
    RecentlyConfirmed,
    Stopped,
}

fn priority_activation_error(error: AecInsertError) -> PriorityActivationResult {
    match error {
        AecInsertError::RecentlyConfirmed => PriorityActivationResult::RecentlyConfirmed,
        AecInsertError::Duplicate => PriorityActivationResult::Duplicate,
        AecInsertError::Stopped => PriorityActivationResult::Stopped,
    }
}

pub struct AecService {
    aec: RwLock<ActiveElectionsContainer>,
    global: RwLock<AecGlobalState>,
    observer: RwLock<Option<Sender<AecFact>>>,
}

struct AecGlobalState {
    stopped: bool,
    count_by_behavior: [usize; ElectionBehavior::COUNT],
    recently_confirmed: RecentlyConfirmedCache,
    cooldown: CooldownController,
    max_elections: usize,
    stats: AecStats,
}

impl AecGlobalState {
    fn new(config: ActiveElectionsConfig) -> Self {
        Self {
            stopped: false,
            count_by_behavior: Default::default(),
            recently_confirmed: RecentlyConfirmedCache::new(config.confirmation_cache),
            cooldown: CooldownController::default(),
            max_elections: config.max_elections,
            stats: Default::default(),
        }
    }

    fn ensure_can_insert(&self, request: &AecInsertRequest) -> Result<(), AecInsertError> {
        if self.stopped {
            return Err(AecInsertError::Stopped);
        }

        if self
            .recently_confirmed
            .root_exists(&request.block.qualified_root())
        {
            return Err(AecInsertError::RecentlyConfirmed);
        }

        Ok(())
    }

    fn count_by_behavior(&self, behavior: ElectionBehavior) -> usize {
        self.count_by_behavior[behavior as usize]
    }

    fn insert_result(&mut self, result: &InsertResult) {
        match result {
            InsertResult::Inserted { behavior, .. } => {
                self.count_by_behavior[*behavior as usize] += 1;
                self.stats.started(*behavior);
            }
            InsertResult::Upgraded {
                previous_behavior,
                new_behavior,
            } => {
                self.count_by_behavior[*previous_behavior as usize] -= 1;
                self.count_by_behavior[*new_behavior as usize] += 1;
            }
        }
    }

    fn vacancy(&self, current_size: usize) -> i64 {
        if self.cooldown.is_cooling_down() {
            return 0;
        }

        self.max_elections as i64 - current_size as i64
    }

    fn info(&self, total: usize) -> ActiveElectionsInfo {
        ActiveElectionsInfo {
            max_elections: self.max_elections,
            total,
            priority: self.count_by_behavior(ElectionBehavior::Priority),
            hinted: self.count_by_behavior(ElectionBehavior::Hinted),
            optimistic: self.count_by_behavior(ElectionBehavior::Optimistic),
        }
    }

    fn cleanup_election(&mut self, election: &Election) {
        self.count_by_behavior[election.behavior() as usize] -= 1;
        self.stats.stopped(election);
    }
}

impl AecService {
    pub fn new(config: ActiveElectionsConfig, base_latency: Duration) -> Self {
        Self {
            aec: RwLock::new(ActiveElectionsContainer::new(base_latency)),
            global: RwLock::new(AecGlobalState::new(config)),
            observer: RwLock::new(None),
        }
    }

    pub fn new_null() -> Self {
        Self {
            aec: RwLock::new(ActiveElectionsContainer::default()),
            global: RwLock::new(AecGlobalState::new(ActiveElectionsConfig::default())),
            observer: RwLock::new(None),
        }
    }

    // --- Read forwarding ---

    pub fn election_for_root(&self, root: &QualifiedRoot) -> Option<Election> {
        self.aec.read().unwrap().election_for_root(root)
    }

    pub fn election_for_block(&self, block_hash: &BlockHash) -> Option<Election> {
        self.aec.read().unwrap().election_for_block(block_hash)
    }

    pub fn max_len(&self) -> usize {
        self.global.read().unwrap().max_elections
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
        self.global.read().unwrap().recently_confirmed.hash_exists(block_hash)
    }

    pub fn count_by_behavior(&self, behavior: ElectionBehavior) -> usize {
        self.global.read().unwrap().count_by_behavior(behavior)
    }

    pub fn bucket_len(&self, bucket_id: usize) -> usize {
        self.aec.read().unwrap().bucket_len(bucket_id)
    }

    pub fn find_bucket(&self, root: &QualifiedRoot) -> Option<usize> {
        self.aec.read().unwrap().find_bucket(root)
    }

    pub fn lowest_priority(&self, bucket_id: usize) -> Option<(QualifiedRoot, TimePriority)> {
        self.aec.read().unwrap().lowest_priority(bucket_id)
    }

    pub fn vacancy(&self) -> i64 {
        let global = self.global.read().unwrap();
        let current_size = self.aec.read().unwrap().len();
        global.vacancy(current_size)
    }

    pub fn info(&self) -> ActiveElectionsInfo {
        let global = self.global.read().unwrap();
        let total = self.aec.read().unwrap().len();
        global.info(total)
    }

    pub fn priority_bucket_available(
        &self,
        bucket_id: usize,
        reserved_elections: usize,
        candidate_prio: TimePriority,
    ) -> bool {
        let global = self.global.read().unwrap();
        let aec = self.aec.read().unwrap();
        let bucket_len = aec.bucket_len(bucket_id);
        let lowest_prio = aec.lowest_priority(bucket_id);

        let can_reprioritize = lowest_prio
            .map(|(_, lowest)| candidate_prio > lowest)
            .unwrap_or(false);

        if can_reprioritize {
            return true;
        }

        if bucket_len >= reserved_elections {
            return false;
        }

        global.vacancy(aec.len()) > 0
    }

    // --- Write forwarding ---

    pub fn set_observer(&self, observer: Sender<AecFact>) {
        let mut current = self.observer.write().unwrap();
        assert!(current.is_none(), "AEC observer already set");
        *current = Some(observer);
    }

    pub fn insert(&self, request: AecInsertRequest, now: Timestamp) -> Result<(), AecInsertError> {
        let inserted = {
            let mut global = self.global.write().unwrap();
            global.ensure_can_insert(&request)?;
            let mut aec = self.aec.write().unwrap();
            let inserted = aec.insert(request, now)?;
            global.insert_result(&inserted);
            inserted
        };

        if let InsertResult::Inserted { hash, root, .. } = inserted {
            self.notify(AecFact::ElectionStarted(hash, root));
        }

        Ok(())
    }

    pub fn activate_priority(
        &self,
        bucket_id: usize,
        reserved_elections: usize,
        block: SavedBlock,
        priority: rsnano_types::BlockPriority,
        now: Timestamp,
    ) -> PriorityActivationResult {
        let root = block.qualified_root();
        let mut ended = None;
        let inserted = {
            let mut global = self.global.write().unwrap();
            if let Err(err) =
                global.ensure_can_insert(&AecInsertRequest::new_priority(block.clone(), priority))
            {
                return priority_activation_error(err);
            }
            let mut aec = self.aec.write().unwrap();

            if aec.find_bucket(&root) == Some(bucket_id) {
                return PriorityActivationResult::Duplicate;
            }

            let replaced = if aec.bucket_len(bucket_id) >= reserved_elections {
                ended = aec.erase_lowest_prio_election(bucket_id);
                true
            } else {
                false
            };

            match aec.insert(AecInsertRequest::new_priority(block, priority), now) {
                Ok(inserted) => {
                    if let Some(election) = &ended {
                        global.cleanup_election(election);
                    }
                    global.insert_result(&inserted);
                    Ok((inserted, replaced))
                }
                Err(err) => Err(priority_activation_error(err)),
            }
        };

        let (inserted, replaced) = match inserted {
            Ok(value) => value,
            Err(result) => return result,
        };

        if let Some(election) = ended {
            self.notify(AecFact::ElectionEnded(election));
        }

        if let InsertResult::Inserted { hash, root, .. } = inserted {
            self.notify(AecFact::ElectionStarted(hash, root));
        }

        if replaced {
            PriorityActivationResult::ActivatedWithReplacement
        } else {
            PriorityActivationResult::Activated
        }
    }

    pub fn try_add_fork(&self, fork: &Block, fork_tally: Amount) -> bool {
        let root = fork.qualified_root();
        let Some(handle) = self.election_handle_for_root(&root) else {
            return false;
        };

        let result = handle.lock().try_add_fork(fork, fork_tally);
        let change = match result {
            AddForkResult::Duplicate | AddForkResult::ElectionEnded => return false,
            result => {
                let mut global = self.global.write().unwrap();
                let mut aec = self.aec.write().unwrap();
                let change = aec.apply_fork_result(&root, &handle, fork, result);
                if matches!(change, ForkChange::Added { .. } | ForkChange::Replaced { .. }) {
                    global.stats.conflicts += 1;
                }
                change
            }
        };

        match change {
            ForkChange::Added { added_hash } => {
                self.notify(AecFact::BlockAddedToElection(added_hash));
                true
            }
            ForkChange::Replaced { added_hash, removed } => {
                self.notify(AecFact::BlockDiscarded(removed));
                self.notify(AecFact::BlockAddedToElection(added_hash));
                true
            }
            ForkChange::Discarded { discarded } => {
                self.notify(AecFact::BlockDiscarded(discarded));
                false
            }
            ForkChange::Ignored => false,
        }
    }

    pub fn set_last_voted(&self, root: &QualifiedRoot, vote_type: VoteType, timestamp: Timestamp) {
        let handle = {
            let aec = self.aec.read().unwrap();
            aec.election_handle_for_root(root)
        };

        if let Some(handle) = handle {
            handle.lock().voted(vote_type, timestamp);
        }
    }

    pub fn apply_vote<'a>(
        &self,
        args: ApplyVoteArgs<'a>,
    ) -> HashMap<BlockHash, Result<(), VoteError>> {
        let mut filtered_blocks = args.vote.filtered_blocks().copied();
        let Some(block_hash) = filtered_blocks.next() else {
            return HashMap::new();
        };

        if filtered_blocks.next().is_none() {
            let observer = self.observer();
            let result = self.resolve_vote_result(block_hash);

            let helper = ApplyVoteHelper {
                args: &args,
                observer,
            };
            let mut results = HashMap::new();

            let (vote_result, counted_vote) = match result {
                ResolvedVoteResult::Apply(handle) => {
                    let apply_result = helper.apply_vote(&handle, &block_hash);
                    if let Some(cleanup) = apply_result.confirmed {
                        self.cleanup_confirmed_election(cleanup.election);
                    }
                    (apply_result.vote_result, apply_result.vote_was_counted)
                }
                ResolvedVoteResult::Resolved(result) => (result, false),
            };

            if counted_vote {
                self.global.write().unwrap().stats.vote_counter.count(args.vote.source);
            }

            results.insert(block_hash, vote_result);
            return results;
        }

        let observer = self.observer();
        let (mut pending_votes, mut results) = {
            let mut pending_votes = Vec::new();
            let mut results = HashMap::new();
            let mut seen = HashSet::new();

            for block_hash in args.vote.filtered_blocks() {
                if !seen.insert(*block_hash) {
                    continue;
                }

                match self.resolve_vote_result(*block_hash) {
                    ResolvedVoteResult::Apply(handle) => pending_votes.push((*block_hash, handle)),
                    ResolvedVoteResult::Resolved(result) => {
                        results.insert(*block_hash, result);
                    }
                }
            }

            (pending_votes, results)
        };

        let helper = ApplyVoteHelper {
            args: &args,
            observer,
        };
        let mut counted_votes = 0;

        for (block_hash, handle) in pending_votes.drain(..) {
            let apply_result = helper.apply_vote(&handle, &block_hash);
            if apply_result.vote_was_counted {
                counted_votes += 1;
            }
            if let Some(cleanup) = apply_result.confirmed {
                self.cleanup_confirmed_election(cleanup.election);
            }
            results.insert(block_hash, apply_result.vote_result);
        }

        if counted_votes > 0 {
            let mut global = self.global.write().unwrap();
            for _ in 0..counted_votes {
                global.stats.vote_counter.count(args.vote.source);
            }
        }

        results
    }

    fn resolve_vote_result(&self, block_hash: BlockHash) -> ResolvedVoteResult {
        let global = self.global.read().unwrap();
        let aec = self.aec.read().unwrap();
        if let Some(handle) = aec.election_handle_for_block(&block_hash) {
            ResolvedVoteResult::Apply(handle)
        } else if global.recently_confirmed.hash_exists(&block_hash) {
            ResolvedVoteResult::Resolved(Err(VoteError::Late))
        } else {
            ResolvedVoteResult::Resolved(Err(VoteError::Indeterminate))
        }
    }

    pub fn transition_time(&self, now: Timestamp) {
        let mut ended = Vec::new();

        self.for_each_round_robin_handle(|_, root, handle| {
            let mut election = handle.lock();
            election.transition_time(now);
            if election.state().has_ended() {
                ended.push(root);
            }
            true
        });

        let ended = {
            let mut global = self.global.write().unwrap();
            global.stats.ticked += 1;
            let mut aec = self.aec.write().unwrap();
            ended
                .into_iter()
                .filter_map(|root| aec.erase(&root))
                .inspect(|election| global.cleanup_election(election))
                .collect::<Vec<_>>()
        };

        for election in ended {
            self.notify(AecFact::ElectionEnded(election));
        }
    }

    pub fn next_vote_in_bucket(
        &self,
        bucket_id: usize,
        vote_broadcast_interval: Duration,
        now: Timestamp,
    ) -> Option<(QualifiedRoot, VoteType, BlockHash)> {
        let mut cursor = None;
        while let Some((next_cursor, root, handle)) =
            self.next_bucket_handle(bucket_id, cursor.as_ref())
        {
            cursor = Some(next_cursor);
            let election = handle.lock();
            if election.can_vote(vote_broadcast_interval, now) {
                return Some((root, election.vote_type(), election.winner().hash()));
            }
        }

        None
    }

    pub fn stale_accounts(
        &self,
        now: Timestamp,
        stale_threshold: Duration,
        limit: usize,
    ) -> Vec<Account> {
        if limit == 0 {
            return Vec::new();
        }

        let mut stale_accounts = Vec::new();

        self.for_each_round_robin_handle(|_, _, handle| {
            let election = handle.lock();
            if election.start().elapsed(now) >= stale_threshold {
                stale_accounts.push(election.account());
            }
            stale_accounts.len() < limit
        });

        stale_accounts
    }

    pub fn election_snapshots(&self) -> Vec<Election> {
        self.aec.read().unwrap().iter_round_robin().collect()
    }

    pub fn active_election_snapshots(&self) -> Vec<Election> {
        self.election_snapshots()
            .into_iter()
            .filter(|e| e.state() == ElectionState::Active)
            .collect()
    }

    pub fn transition_active(&self, block_hash: &BlockHash) -> bool {
        let Some(handle) = self.election_handle_for_block(block_hash) else {
            return false;
        };
        handle.lock().transition_active();
        true
    }

    pub fn remove_votes<'a>(
        &self,
        root: &QualifiedRoot,
        voters: impl IntoIterator<Item = &'a PublicKey>,
    ) {
        let Some(handle) = self.election_handle_for_root(root) else {
            return;
        };

        let mut election = handle.lock();
        for voter in voters {
            election.remove_vote(voter);
        }
    }

    pub fn erase_ended_elections(&self) {
        let ended = {
            let mut global = self.global.write().unwrap();
            let mut aec = self.aec.write().unwrap();
            let ended = aec.take_ended_elections();
            for election in &ended {
                global.cleanup_election(election);
            }
            ended
        };

        for election in ended {
            self.notify(AecFact::ElectionEnded(election));
        }
    }

    pub fn erase(&self, root: &QualifiedRoot) -> bool {
        let ended = {
            let mut global = self.global.write().unwrap();
            let mut aec = self.aec.write().unwrap();
            let ended = aec.erase(root);
            if let Some(election) = &ended {
                global.cleanup_election(election);
            }
            ended
        };

        if let Some(election) = ended {
            self.notify(AecFact::ElectionEnded(election));
            true
        } else {
            false
        }
    }

    pub fn erase_lowest_prio_election(&self, bucket_id: usize) {
        if let Some(election) = {
            let mut global = self.global.write().unwrap();
            let mut aec = self.aec.write().unwrap();
            let election = aec.erase_lowest_prio_election(bucket_id);
            if let Some(election) = &election {
                global.cleanup_election(election);
            }
            election
        } {
            self.notify(AecFact::ElectionEnded(election));
        }
    }

    pub fn confirm_dependent_elections(
        &self,
        confirmed: Vec<(SavedBlock, Option<ConfirmedElection>)>,
        now: Timestamp,
    ) {
        let mut confirmed_results = Vec::with_capacity(confirmed.len());

        for (confirmed_block, source_election) in confirmed {
            let confirmed_election = if let Some(source) = source_election
                && confirmed_block.hash() == source.winner.hash()
            {
                source
            } else if let Some(handle) =
                self.election_handle_for_root(&confirmed_block.qualified_root())
            {
                let mut election = handle.lock();
                if election.winner().hash() == confirmed_block.hash() {
                    election.force_confirm();
                    election
                        .into_confirmed_election(now, ConfirmationType::ActiveConfirmationHeight)
                } else {
                    election.cancel();
                    ConfirmedElection::new(
                        confirmed_block.clone(),
                        ConfirmationType::ActiveConfirmationHeight,
                    )
                }
            } else {
                ConfirmedElection::new(
                    confirmed_block.clone(),
                    ConfirmationType::InactiveConfirmationHeight,
                )
            };

            confirmed_results.push((confirmed_block, confirmed_election));
        }

        {
            let mut global = self.global.write().unwrap();
            for (_, election) in &confirmed_results {
                global.stats.block_confirmations[election.confirmation_type as usize] += 1;
            }
        }
        for (block, election) in confirmed_results {
            self.notify(AecFact::BlockConfirmed(block, election));
        }
    }

    pub fn remove_recently_confirmed(&self, block_hash: &BlockHash) {
        self.global
            .write()
            .unwrap()
            .recently_confirmed
            .erase(block_hash)
    }

    pub fn set_cooldown(&self, cool_down: bool, reason: AecCooldownReason) {
        let recovered = {
            self.global
                .write()
                .unwrap()
                .cooldown
                .set_cooldown(cool_down, reason)
                == CooldownResult::Recovered
        };

        if recovered {
            self.notify(AecFact::Recovered);
        }
    }

    pub fn cancel(&self, root: &QualifiedRoot) {
        let Some(handle) = self.election_handle_for_root(root) else {
            return;
        };
        handle.lock().cancel();
    }

    pub fn cancel_all(&self) {
        self.aec.write().unwrap().cancel_all()
    }

    pub fn clear_recently_confirmed(&self) {
        self.global.write().unwrap().recently_confirmed.clear()
    }

    pub fn stop(&self) {
        let _ = self.observer.write().unwrap().take();
        let mut global = self.global.write().unwrap();
        global.stopped = true;
        global.count_by_behavior = Default::default();
        let mut aec = self.aec.write().unwrap();
        let _ = aec.stop();
    }

    pub fn force_confirm(&self, block_hash: &BlockHash, now: Timestamp) {
        let handle = self
            .aec
            .read()
            .unwrap()
            .election_handle_for_block(block_hash)
            .unwrap_or_else(|| {
                panic!("Force confirm failed, because no active election was found")
            });

        let confirmed_election = {
            let mut election = handle.lock();
            if !election.force_confirm() {
                return;
            }
            election.into_confirmed_election(now, ConfirmationType::ActiveConfirmedQuorum)
        };

        self.notify(AecFact::ElectionConfirmed(confirmed_election));
    }

    pub fn simulate_event(&self, event: AecFact) {
        self.notify(event)
    }

    fn cleanup_confirmed_election(&self, election: Election) {
        let ended = {
            let mut global = self.global.write().unwrap();
            global
                .recently_confirmed
                .put(election.qualified_root().clone(), election.winner().hash());
            let mut aec = self.aec.write().unwrap();
            if aec.erase_with_known_election(election.qualified_root(), &election) {
                global.cleanup_election(&election);
                Some(election)
            } else {
                None
            }
        };

        if let Some(election) = ended {
            self.notify(AecFact::ElectionEnded(election));
        }
    }

    fn observer(&self) -> Option<Sender<AecFact>> {
        self.observer.read().unwrap().clone()
    }

    fn notify(&self, event: AecFact) {
        if let Some(observer) = self.observer() {
            observer.send(event).unwrap();
        }
    }

    fn election_handle_for_block(&self, block_hash: &BlockHash) -> Option<ElectionHandle> {
        self.aec
            .read()
            .unwrap()
            .election_handle_for_block(block_hash)
    }

    fn election_handle_for_root(&self, root: &QualifiedRoot) -> Option<ElectionHandle> {
        self.aec.read().unwrap().election_handle_for_root(root)
    }

    fn next_bucket_handle(
        &self,
        bucket_id: usize,
        after: Option<&BucketCursor>,
    ) -> Option<(BucketCursor, QualifiedRoot, ElectionHandle)> {
        let aec = self.aec.read().unwrap();
        aec.next_bucket(bucket_id, after)
            .map(|(cursor, (root, handle))| (cursor, root, handle))
    }

    fn for_each_round_robin_handle(
        &self,
        mut f: impl FnMut(usize, QualifiedRoot, ElectionHandle) -> bool,
    ) {
        let mut cursors = vec![None; bucket_count()];
        let mut next_bucket = bucket_count().saturating_sub(1);

        while let Some((bucket_id, cursor, root, handle)) =
            self.next_round_robin_handle(&cursors, next_bucket)
        {
            cursors[bucket_id] = Some(cursor);
            next_bucket = bucket_id.checked_sub(1).unwrap_or(bucket_count() - 1);
            if !f(bucket_id, root, handle) {
                break;
            }
        }
    }

    fn next_round_robin_handle(
        &self,
        cursors: &[Option<BucketCursor>],
        start_bucket: usize,
    ) -> Option<(usize, BucketCursor, QualifiedRoot, ElectionHandle)> {
        for offset in 0..bucket_count() {
            let bucket_id = (start_bucket + bucket_count() - offset) % bucket_count();
            if let Some((cursor, root, handle)) =
                self.next_bucket_handle(bucket_id, cursors[bucket_id].as_ref())
            {
                return Some((bucket_id, cursor, root, handle));
            }
        }
        None
    }
}

impl StatsSource for AecService {
    fn collect_stats(&self, result: &mut StatsCollection) {
        let global = self.global.read().unwrap();
        global.cooldown.collect_stats(result);
        global.stats.collect_stats(result);
    }
}

impl ContainerInfoProvider for AecService {
    fn container_info(&self) -> ContainerInfo {
        let global = self.global.read().unwrap();
        let aec = self.aec.read().unwrap();
        ContainerInfo::builder()
            .leaf("roots", aec.len(), RootContainer::ELEMENT_SIZE)
            .leaf(
                "normal",
                global.count_by_behavior(ElectionBehavior::Priority),
                0,
            )
            .leaf(
                "hinted".to_string(),
                global.count_by_behavior(ElectionBehavior::Hinted),
                0,
            )
            .leaf(
                "optimistic".to_string(),
                global.count_by_behavior(ElectionBehavior::Optimistic),
                0,
            )
            .node(
                "recently_confirmed",
                global.recently_confirmed.container_info(),
            )
            .node("vote_router", aec.vote_router_container_info())
            .finish()
    }
}

enum ResolvedVoteResult {
    Apply(ElectionHandle),
    Resolved(Result<(), VoteError>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        consensus::{AecInsertRequest, ReceivedVote},
        representatives::QuorumSpecs,
    };
    use rsnano_ledger::RepWeights;
    use rsnano_types::{BlockPriority, PrivateKey, SavedBlock, Vote, VoteSource};
    use rsnano_utils::sync::backpressure_channel::channel;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };

    #[test]
    fn apply_vote_does_not_hold_container_write_lock_while_waiting_for_other_election() {
        let aec = Arc::new(AecService::new_null());
        let block_a = SavedBlock::new_test_instance_with_key(1);
        let block_b = SavedBlock::new_test_instance_with_key(2);
        let now = Timestamp::new_test_instance();

        aec.insert(
            AecInsertRequest::new_priority(block_a.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();
        aec.insert(
            AecInsertRequest::new_priority(block_b.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        let locked_handle = aec
            .aec
            .read()
            .unwrap()
            .election_handle_for_block(&block_a.hash())
            .unwrap();
        let election_guard = locked_handle.lock();

        let rep_key = PrivateKey::from(1);
        let mut rep_weights = RepWeights::default();
        rep_weights.put(rep_key.public_key(), Amount::MAX);
        let quorum_specs = QuorumSpecs::new_test_instance();
        let started = Arc::new(AtomicBool::new(false));
        let started_clone = started.clone();
        let aec_for_thread = aec.clone();
        let vote_a: ReceivedVote = ReceivedVote::new(
            Vote::new_final(&rep_key, vec![block_a.hash()]).into(),
            VoteSource::Live,
            None,
        );

        let worker = thread::spawn(move || {
            started_clone.store(true, Ordering::Release);
            aec_for_thread.apply_vote(ApplyVoteArgs {
                vote: &vote_a.into(),
                rep_weights: &rep_weights,
                quorum_specs: &quorum_specs,
                now,
            })
        });

        while !started.load(Ordering::Acquire) {
            thread::yield_now();
        }
        let mut container_write_available = false;
        for _ in 0..10_000 {
            if worker.is_finished() {
                break;
            }
            if aec.aec.try_write().is_ok() {
                container_write_available = true;
                break;
            }
            thread::yield_now();
        }

        assert!(container_write_available);

        let mut rep_weights_b = RepWeights::default();
        rep_weights_b.put(rep_key.public_key(), Amount::MAX);
        let vote_b: ReceivedVote = ReceivedVote::new(
            Vote::new_final(&rep_key, vec![block_b.hash()]).into(),
            VoteSource::Live,
            None,
        );
        let results = aec.apply_vote(ApplyVoteArgs {
            vote: &vote_b.into(),
            rep_weights: &rep_weights_b,
            quorum_specs: &QuorumSpecs::new_test_instance(),
            now,
        });

        assert_eq!(results.get(&block_b.hash()), Some(&Ok(())));

        drop(election_guard);
        let first_results = worker.join().unwrap();
        assert_eq!(first_results.get(&block_a.hash()), Some(&Ok(())));
        assert!(aec.was_recently_confirmed(&block_a.hash()));
        assert!(aec.was_recently_confirmed(&block_b.hash()));
    }

    #[test]
    fn apply_vote_removes_confirmed_election_immediately_after_vote() {
        let config = ActiveElectionsConfig {
            max_elections: 1,
            ..Default::default()
        };
        let aec = AecService::new(config, Duration::from_secs(1));
        let block = SavedBlock::new_test_instance();
        let now = Timestamp::new_test_instance();

        aec.insert(
            AecInsertRequest::new_priority(block.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        let rep_key = PrivateKey::from(1);
        let mut rep_weights = RepWeights::default();
        rep_weights.put(rep_key.public_key(), Amount::MAX);
        let vote: ReceivedVote = ReceivedVote::new(
            Vote::new_final(&rep_key, vec![block.hash()]).into(),
            VoteSource::Live,
            None,
        );

        let results = aec.apply_vote(ApplyVoteArgs {
            vote: &vote.into(),
            rep_weights: &rep_weights,
            quorum_specs: &QuorumSpecs::new_test_instance(),
            now,
        });

        assert_eq!(results.get(&block.hash()), Some(&Ok(())));
        assert!(!aec.is_active_root(&block.qualified_root()));
        assert!(aec.was_recently_confirmed(&block.hash()));
        assert_eq!(aec.vacancy(), 1);
    }

    #[test]
    fn info_tracks_service_owned_behavior_counts() {
        let config = ActiveElectionsConfig {
            max_elections: 2,
            ..Default::default()
        };
        let aec = AecService::new(config, Duration::from_secs(1));
        let block = SavedBlock::new_test_instance();
        let now = Timestamp::new_test_instance();

        aec.insert(
            AecInsertRequest::new_hinted(block.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        let info = aec.info();
        assert_eq!(info.max_elections, 2);
        assert_eq!(info.total, 1);
        assert_eq!(info.hinted, 1);
        assert_eq!(aec.count_by_behavior(ElectionBehavior::Hinted), 1);
        assert_eq!(aec.vacancy(), 1);

        assert!(aec.erase(&block.qualified_root()));

        let info = aec.info();
        assert_eq!(info.total, 0);
        assert_eq!(info.hinted, 0);
        assert_eq!(aec.count_by_behavior(ElectionBehavior::Hinted), 0);
    }

    #[test]
    fn cooldown_controls_service_owned_vacancy_and_recovery_event() {
        let aec = AecService::new_null();
        let block = SavedBlock::new_test_instance();
        let now = Timestamp::new_test_instance();
        let (tx, rx) = channel(1);

        aec.set_observer(tx);
        aec.insert(
            AecInsertRequest::new_priority(block, BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        assert_eq!(aec.vacancy(), 4999);

        aec.set_cooldown(true, AecCooldownReason::ConfirmingSetFull);
        assert_eq!(aec.vacancy(), 0);

        aec.set_cooldown(false, AecCooldownReason::ConfirmingSetFull);
        assert!(matches!(rx.recv().unwrap(), AecFact::ElectionStarted(_, _)));
        assert!(matches!(rx.recv().unwrap(), AecFact::Recovered));
    }

    #[test]
    fn apply_vote_preserves_confirmed_election_ended_observer_payload() {
        let aec = AecService::new_null();
        let block = SavedBlock::new_test_instance();
        let now = Timestamp::new_test_instance();
        let (tx, rx) = channel(8);

        aec.set_observer(tx);
        aec.insert(
            AecInsertRequest::new_priority(block.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        let rep_key = PrivateKey::from(1);
        let mut rep_weights = RepWeights::default();
        rep_weights.put(rep_key.public_key(), Amount::MAX);
        let vote: ReceivedVote = ReceivedVote::new(
            Vote::new_final(&rep_key, vec![block.hash()]).into(),
            VoteSource::Live,
            None,
        );

        let results = aec.apply_vote(ApplyVoteArgs {
            vote: &vote.into(),
            rep_weights: &rep_weights,
            quorum_specs: &QuorumSpecs::new_test_instance(),
            now,
        });

        assert_eq!(results.get(&block.hash()), Some(&Ok(())));

        let mut confirmed_seen = false;
        let mut ended_seen = false;
        for _ in 0..3 {
            match rx.recv().unwrap() {
                AecFact::ElectionStarted(_, _) => {}
                AecFact::ElectionConfirmed(confirmed) => {
                    confirmed_seen = true;
                    assert_eq!(confirmed.winner.hash(), block.hash());
                }
                AecFact::ElectionEnded(election) => {
                    ended_seen = true;
                    assert_eq!(election.qualified_root(), &block.qualified_root());
                    assert_eq!(election.winner().hash(), block.hash());
                    assert!(election.is_confirmed());
                }
                _ => panic!("unexpected event"),
            }
        }

        assert!(confirmed_seen);
        assert!(ended_seen);
    }

    #[test]
    fn apply_vote_returns_indeterminate_for_single_missing_hash() {
        let aec = AecService::new_null();
        let block = SavedBlock::new_test_instance();
        let now = Timestamp::new_test_instance();

        let vote: ReceivedVote = ReceivedVote::new(
            Vote::new_final(&PrivateKey::from(1), vec![block.hash()]).into(),
            VoteSource::Live,
            None,
        );

        let results = aec.apply_vote(ApplyVoteArgs {
            vote: &vote.into(),
            rep_weights: &RepWeights::default(),
            quorum_specs: &QuorumSpecs::new_test_instance(),
            now,
        });

        assert_eq!(results.len(), 1);
        assert_eq!(
            results.get(&block.hash()),
            Some(&Err(VoteError::Indeterminate))
        );
    }

    #[test]
    fn apply_vote_ignores_duplicate_single_hash_entries() {
        let aec = AecService::new_null();
        let block = SavedBlock::new_test_instance();
        let now = Timestamp::new_test_instance();

        aec.insert(
            AecInsertRequest::new_priority(block.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        let rep_key = PrivateKey::from(1);
        let mut rep_weights = RepWeights::default();
        rep_weights.put(rep_key.public_key(), Amount::MAX);
        let vote: ReceivedVote = ReceivedVote::new(
            Vote::new_final(&rep_key, vec![block.hash(), block.hash()]).into(),
            VoteSource::Live,
            None,
        );

        let results = aec.apply_vote(ApplyVoteArgs {
            vote: &vote.into(),
            rep_weights: &rep_weights,
            quorum_specs: &QuorumSpecs::new_test_instance(),
            now,
        });

        assert_eq!(results.len(), 1);
        assert_eq!(results.get(&block.hash()), Some(&Ok(())));
        assert!(!aec.is_active_root(&block.qualified_root()));
        assert!(aec.was_recently_confirmed(&block.hash()));
    }

    #[test]
    fn transition_active_does_not_hold_container_write_lock_while_waiting_for_other_election() {
        let aec = Arc::new(AecService::new_null());
        let block_a = SavedBlock::new_test_instance_with_key(1);
        let block_b = SavedBlock::new_test_instance_with_key(2);
        let now = Timestamp::new_test_instance();

        aec.insert(
            AecInsertRequest::new_priority(block_a.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();
        aec.insert(
            AecInsertRequest::new_priority(block_b.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        let locked_handle = aec
            .aec
            .read()
            .unwrap()
            .election_handle_for_block(&block_a.hash())
            .unwrap();
        let election_guard = locked_handle.lock();

        let started = Arc::new(AtomicBool::new(false));
        let started_clone = Arc::clone(&started);
        let aec_for_thread = Arc::clone(&aec);
        let block_hash = block_b.hash();

        let worker = thread::spawn(move || {
            started_clone.store(true, Ordering::Release);
            assert!(aec_for_thread.transition_active(&block_hash));
        });

        while !started.load(Ordering::Acquire) {
            thread::yield_now();
        }

        let mut container_write_available = false;
        for _ in 0..10_000 {
            if worker.is_finished() {
                break;
            }
            if aec.aec.try_write().is_ok() {
                container_write_available = true;
                break;
            }
            thread::yield_now();
        }

        assert!(container_write_available);

        drop(election_guard);
        worker.join().unwrap();
        assert_eq!(
            aec.election_for_block(&block_b.hash()).unwrap().state(),
            crate::consensus::election::ElectionState::Active
        );
    }

    #[test]
    fn confirm_dependent_elections_does_not_hold_container_write_lock_while_waiting_for_election() {
        let aec = Arc::new(AecService::new_null());
        let block = SavedBlock::new_test_instance_with_key(1);
        let now = Timestamp::new_test_instance();

        aec.insert(
            AecInsertRequest::new_priority(block.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        let (tx, rx) = channel(1);
        aec.set_observer(tx);

        let locked_handle = aec
            .aec
            .read()
            .unwrap()
            .election_handle_for_root(&block.qualified_root())
            .unwrap();
        let election_guard = locked_handle.lock();

        let started = Arc::new(AtomicBool::new(false));
        let started_clone = Arc::clone(&started);
        let aec_for_thread = Arc::clone(&aec);
        let confirmed = vec![(block.clone(), None)];

        let worker = thread::spawn(move || {
            started_clone.store(true, Ordering::Release);
            aec_for_thread.confirm_dependent_elections(confirmed, now);
        });

        while !started.load(Ordering::Acquire) {
            thread::yield_now();
        }

        let mut container_write_available = false;
        for _ in 0..10_000 {
            if worker.is_finished() {
                break;
            }
            if aec.aec.try_write().is_ok() {
                container_write_available = true;
                break;
            }
            thread::yield_now();
        }

        assert!(container_write_available);

        drop(election_guard);
        worker.join().unwrap();

        let event = rx.recv().unwrap();
        let AecFact::BlockConfirmed(confirmed_block, confirmed_election) = event else {
            panic!("expected BlockConfirmed event");
        };
        assert_eq!(confirmed_block.hash(), block.hash());
        assert_eq!(
            confirmed_election.confirmation_type,
            ConfirmationType::ActiveConfirmationHeight
        );
    }

    #[test]
    fn stale_accounts_streams_round_robin_order_with_limit() {
        let aec = AecService::new_null();
        let now = Timestamp::new_test_instance();
        let stale_start = now - Duration::from_secs(120);
        let blocks = [
            SavedBlock::new_test_instance_with_key(1),
            SavedBlock::new_test_instance_with_key(2),
            SavedBlock::new_test_instance_with_key(3),
        ];

        aec.insert(
            AecInsertRequest::new_priority(blocks[0].clone(), BlockPriority::new_test_instance()),
            stale_start,
        )
        .unwrap();
        aec.insert(
            AecInsertRequest::new_hinted(blocks[1].clone(), BlockPriority::new_test_instance()),
            stale_start,
        )
        .unwrap();
        aec.insert(
            AecInsertRequest::new_manual(blocks[2].clone(), BlockPriority::new_test_instance()),
            stale_start,
        )
        .unwrap();

        let expected: Vec<_> = aec
            .election_snapshots()
            .into_iter()
            .map(|election| election.account())
            .take(2)
            .collect();

        let stale_accounts = aec.stale_accounts(now, Duration::from_secs(60), 2);

        assert_eq!(stale_accounts, expected);
    }

    #[test]
    fn next_vote_in_bucket_streams_until_voteable_election() {
        let aec = AecService::new_null();
        let now = Timestamp::new_test_instance();
        let block_a = SavedBlock::new_test_instance_with_key(1);
        let block_b = SavedBlock::new_test_instance_with_key(2);
        let priority = BlockPriority::new_test_instance();

        aec.insert(
            AecInsertRequest::new_priority(block_a.clone(), priority),
            now - Duration::from_secs(10),
        )
        .unwrap();
        aec.insert(
            AecInsertRequest::new_priority(block_b.clone(), priority),
            now - Duration::from_secs(10),
        )
        .unwrap();

        let bucket_id = aec.find_bucket(&block_a.qualified_root()).unwrap();
        let vote_interval = Duration::from_secs(5);
        aec.set_last_voted(&block_a.qualified_root(), VoteType::NonFinal, now);

        let next_vote = aec
            .next_vote_in_bucket(bucket_id, vote_interval, now)
            .unwrap();

        assert_eq!(next_vote.0, block_b.qualified_root());
        assert_eq!(next_vote.1, VoteType::NonFinal);
        assert_eq!(next_vote.2, block_b.hash());
    }

    #[test]
    fn transition_time_streams_and_erases_ended_elections() {
        let aec = AecService::new_null();
        let now = Timestamp::new_test_instance();
        let transition_at = now + Duration::from_secs(31);
        let block_a = SavedBlock::new_test_instance_with_key(1);
        let block_b = SavedBlock::new_test_instance_with_key(2);

        aec.insert(
            AecInsertRequest::new_hinted(block_a.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();
        aec.insert(
            AecInsertRequest::new_optimistic(block_b.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        aec.transition_time(transition_at);

        assert!(!aec.is_active_root(&block_a.qualified_root()));
        assert!(!aec.is_active_root(&block_b.qualified_root()));
        assert!(aec.election_snapshots().is_empty());
    }
}
