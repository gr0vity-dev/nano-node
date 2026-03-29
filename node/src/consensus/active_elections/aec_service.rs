use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        RwLock, RwLockWriteGuard,
        atomic::{AtomicPtr, Ordering},
    },
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
    AecFact, AecInsertError, AecInsertRequest, ApplyVoteArgs, RootContainer,
    active_elections_container::{ForkChange, InsertResult},
    apply_vote_helper::ApplyVoteHelper,
    cooldown_controller::{CooldownController, CooldownResult},
    recently_confirmed_cache::RecentlyConfirmedCache,
    root_container::{BucketCursor, ElectionHandle, RootedElectionHandle},
    stats::{AecStats, VoteCounter},
    vote_router::VoteRouter,
};
use crate::consensus::election::{
    AddForkResult, ConfirmationType, ConfirmedElection, Election, ElectionBehavior, ElectionState,
    VoteType,
};
use crate::consensus::election_schedulers::priority::bucket_count;
use rustc_hash::FxHashMap;

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
    router: RwLock<VoteRouter>,
    shards: Vec<RwLock<AecShardState>>,
    published_handles: AtomicArc<FxHashMap<QualifiedRoot, ElectionHandle>>,
    lifecycle: RwLock<AecLifecycleState>,
    stats: RwLock<AecStats>,
    vote_counter: VoteCounter,
    observer: RwLock<Option<Sender<AecFact>>>,
}

const AEC_SHARD_COUNT: usize = 8;

struct AecLifecycleState {
    stopped: bool,
    cooldown: CooldownController,
    max_elections: usize,
}

impl AecLifecycleState {
    fn new(config: ActiveElectionsConfig) -> Self {
        Self {
            stopped: false,
            cooldown: CooldownController::default(),
            max_elections: config.max_elections,
        }
    }

    fn ensure_not_stopped(&self) -> Result<(), AecInsertError> {
        if self.stopped {
            return Err(AecInsertError::Stopped);
        }
        Ok(())
    }

    fn vacancy(&self, current_size: usize) -> i64 {
        if self.cooldown.is_cooling_down() {
            return 0;
        }

        self.max_elections as i64 - current_size as i64
    }
}

struct AecShardState {
    elections: ActiveElectionsContainer,
    count_by_behavior: [usize; ElectionBehavior::COUNT],
    recently_confirmed: RecentlyConfirmedCache,
    cleanup_stats: AecStats,
}

impl AecShardState {
    fn new(base_latency: Duration, recently_confirmed_max_len: usize) -> Self {
        Self {
            elections: ActiveElectionsContainer::new(base_latency),
            count_by_behavior: Default::default(),
            recently_confirmed: RecentlyConfirmedCache::new(recently_confirmed_max_len),
            cleanup_stats: AecStats::default(),
        }
    }

    fn insert(
        &mut self,
        request: AecInsertRequest,
        now: Timestamp,
    ) -> Result<InsertResult, AecInsertError> {
        let inserted = self.elections.insert(request, now)?;
        self.insert_result(&inserted);
        Ok(inserted)
    }

    fn insert_result(&mut self, result: &InsertResult) {
        match result {
            InsertResult::Inserted { behavior, .. } => {
                self.count_by_behavior[*behavior as usize] += 1;
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

    fn count_by_behavior(&self, behavior: ElectionBehavior) -> usize {
        self.count_by_behavior[behavior as usize]
    }

    fn cleanup_election(&mut self, election: &Election) {
        self.count_by_behavior[election.behavior() as usize] -= 1;
    }

    fn len(&self) -> usize {
        self.elections.len()
    }

    fn bucket_len(&self, bucket_id: usize) -> usize {
        self.elections.bucket_len(bucket_id)
    }

    fn find_bucket(&self, root: &QualifiedRoot) -> Option<usize> {
        self.elections.find_bucket(root)
    }

    fn lowest_priority(&self, bucket_id: usize) -> Option<(QualifiedRoot, TimePriority)> {
        self.elections.lowest_priority(bucket_id)
    }

    fn election_handle_for_root(&self, root: &QualifiedRoot) -> Option<ElectionHandle> {
        self.elections.election_handle_for_root(root)
    }

    fn next_bucket(
        &self,
        bucket_id: usize,
        after: Option<&BucketCursor>,
    ) -> Option<(BucketCursor, RootedElectionHandle)> {
        self.elections.next_bucket(bucket_id, after)
    }

    fn apply_fork_result(
        &mut self,
        root: &QualifiedRoot,
        handle: &ElectionHandle,
        fork: &Block,
        result: AddForkResult,
    ) -> ForkChange {
        self.elections.apply_fork_result(root, handle, fork, result)
    }

    fn take_ended_elections(&mut self) -> Vec<Election> {
        let ended = self.elections.take_ended_elections();
        for election in &ended {
            self.cleanup_election(election);
        }
        ended
    }

    fn erase(&mut self, root: &QualifiedRoot) -> Option<Election> {
        let ended = self.elections.erase(root);
        if let Some(election) = &ended {
            self.cleanup_election(election);
        }
        ended
    }

    fn erase_with_known_election(&mut self, root: &QualifiedRoot, election: &Election) -> bool {
        let erased = self.elections.erase_with_known_election(root, election);
        if erased {
            self.cleanup_election(election);
        }
        erased
    }

    fn recently_confirmed_root_exists(&self, root: &QualifiedRoot) -> bool {
        self.recently_confirmed.root_exists(root)
    }

    fn recently_confirmed_hash_exists(&self, block_hash: &BlockHash) -> bool {
        self.recently_confirmed.hash_exists(block_hash)
    }

    fn put_recently_confirmed(&mut self, root: QualifiedRoot, hash: BlockHash) {
        self.recently_confirmed.put(root, hash);
    }

    fn remove_recently_confirmed(&mut self, block_hash: &BlockHash) {
        self.recently_confirmed.erase(block_hash);
    }

    fn clear_recently_confirmed(&mut self) {
        self.recently_confirmed.clear();
    }

    fn recently_confirmed_len(&self) -> usize {
        self.recently_confirmed.len()
    }

    fn record_cleanup_stopped(&mut self, election: &Election) {
        self.cleanup_stats.stopped(election);
    }

    fn stop(&mut self) -> Vec<Election> {
        self.count_by_behavior = Default::default();
        self.elections.stop()
    }

    fn cancel_all(&mut self) {
        self.elections.cancel_all();
    }

    fn iter_round_robin(&self) -> impl Iterator<Item = Election> {
        self.elections.iter_round_robin()
    }
}

impl AecService {
    fn stats_inserted(&self, result: &InsertResult) {
        if let InsertResult::Inserted { behavior, .. } = result {
            self.stats.write().unwrap().started(*behavior);
        }
    }

    fn confirmation_cache_len_per_shard(confirmation_cache: usize) -> usize {
        confirmation_cache.div_ceil(AEC_SHARD_COUNT)
    }

    pub fn new(config: ActiveElectionsConfig, base_latency: Duration) -> Self {
        let confirmation_cache = config.confirmation_cache;
        let recently_confirmed_max_len = Self::confirmation_cache_len_per_shard(confirmation_cache);
        Self {
            router: RwLock::new(VoteRouter::default()),
            shards: (0..AEC_SHARD_COUNT)
                .map(|_| RwLock::new(AecShardState::new(base_latency, recently_confirmed_max_len)))
                .collect(),
            published_handles: AtomicArc::new(Arc::new(FxHashMap::default())),
            lifecycle: RwLock::new(AecLifecycleState::new(config)),
            stats: RwLock::new(AecStats::default()),
            vote_counter: VoteCounter::default(),
            observer: RwLock::new(None),
        }
    }

    pub fn new_null() -> Self {
        let recently_confirmed_max_len =
            Self::confirmation_cache_len_per_shard(ActiveElectionsConfig::default().confirmation_cache);
        Self {
            router: RwLock::new(VoteRouter::default()),
            shards: (0..AEC_SHARD_COUNT)
                .map(|_| {
                    RwLock::new(AecShardState::new(
                        Duration::from_secs(0),
                        recently_confirmed_max_len,
                    ))
                })
                .collect(),
            published_handles: AtomicArc::new(Arc::new(FxHashMap::default())),
            lifecycle: RwLock::new(AecLifecycleState::new(ActiveElectionsConfig::default())),
            stats: RwLock::new(AecStats::default()),
            vote_counter: VoteCounter::default(),
            observer: RwLock::new(None),
        }
    }

    // --- Read forwarding ---

    pub fn election_for_root(&self, root: &QualifiedRoot) -> Option<Election> {
        let handle = self.election_handle_for_root(root)?;
        Some(handle.lock().clone())
    }

    pub fn election_for_block(&self, block_hash: &BlockHash) -> Option<Election> {
        let root = self
            .router
            .read()
            .unwrap()
            .qualified_root(block_hash)
            .cloned()?;
        self.election_for_root(&root)
    }

    pub fn max_len(&self) -> usize {
        self.lifecycle.read().unwrap().max_elections
    }

    pub fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.read().unwrap().len())
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_active_root(&self, root: &QualifiedRoot) -> bool {
        self.election_handle_for_root(root).is_some()
    }

    pub fn is_active_hash(&self, block_hash: &BlockHash) -> bool {
        self.router.read().unwrap().is_active(block_hash)
    }

    pub fn was_recently_confirmed(&self, block_hash: &BlockHash) -> bool {
        self.shards.iter().any(|shard| {
            shard.read()
                .unwrap()
                .recently_confirmed_hash_exists(block_hash)
        })
    }

    pub fn count_by_behavior(&self, behavior: ElectionBehavior) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.read().unwrap().count_by_behavior(behavior))
            .sum()
    }

    pub fn bucket_len(&self, bucket_id: usize) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.read().unwrap().bucket_len(bucket_id))
            .sum()
    }

    pub fn find_bucket(&self, root: &QualifiedRoot) -> Option<usize> {
        self.shard(root).read().unwrap().find_bucket(root)
    }

    pub fn lowest_priority(&self, bucket_id: usize) -> Option<(QualifiedRoot, TimePriority)> {
        self.shards
            .iter()
            .filter_map(|shard| shard.read().unwrap().lowest_priority(bucket_id))
            .min_by_key(|(_, priority)| *priority)
    }

    pub fn vacancy(&self) -> i64 {
        let lifecycle = self.lifecycle.read().unwrap();
        let current_size = self.len();
        lifecycle.vacancy(current_size)
    }

    pub fn info(&self) -> ActiveElectionsInfo {
        let total = self.len();
        ActiveElectionsInfo {
            max_elections: self.max_len(),
            total,
            priority: self.count_by_behavior(ElectionBehavior::Priority),
            hinted: self.count_by_behavior(ElectionBehavior::Hinted),
            optimistic: self.count_by_behavior(ElectionBehavior::Optimistic),
        }
    }

    pub fn priority_bucket_available(
        &self,
        bucket_id: usize,
        reserved_elections: usize,
        candidate_prio: TimePriority,
    ) -> bool {
        let lifecycle = self.lifecycle.read().unwrap();
        let bucket_len = self.bucket_len(bucket_id);
        let lowest_prio = self.lowest_priority(bucket_id);

        let can_reprioritize = lowest_prio
            .map(|(_, lowest)| candidate_prio > lowest)
            .unwrap_or(false);

        if can_reprioritize {
            return true;
        }

        if bucket_len >= reserved_elections {
            return false;
        }

        lifecycle.vacancy(self.len()) > 0
    }

    // --- Write forwarding ---

    pub fn set_observer(&self, observer: Sender<AecFact>) {
        let mut current = self.observer.write().unwrap();
        assert!(current.is_none(), "AEC observer already set");
        *current = Some(observer);
    }

    pub fn insert(&self, request: AecInsertRequest, now: Timestamp) -> Result<(), AecInsertError> {
        let shard_index = self.shard_index_for_root(&request.block.qualified_root());
        let inserted = {
            let lifecycle = self.lifecycle.read().unwrap();
            lifecycle.ensure_not_stopped()?;
            let mut shard = self.shards[shard_index].write().unwrap();
            if shard.recently_confirmed_root_exists(&request.block.qualified_root()) {
                return Err(AecInsertError::RecentlyConfirmed);
            }
            let inserted = shard.insert(request, now)?;
            if let Some(root) = inserted.inserted_root() {
                self.publish_shard_handle(root, &shard);
            }
            self.stats_inserted(&inserted);
            inserted
        };

        if let InsertResult::Inserted { hash, root, .. } = inserted {
            self.router.write().unwrap().connect(hash, root.clone());
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
        let insert_shard_index = self.shard_index_for_root(&root);
        let replacement_root = if self.bucket_len(bucket_id) >= reserved_elections {
            self.lowest_priority(bucket_id).map(|(root, _)| root)
        } else {
            None
        };
        let mut ended = None;
        let inserted = {
            let lifecycle = self.lifecycle.read().unwrap();
            if let Err(err) = lifecycle.ensure_not_stopped() {
                return priority_activation_error(err);
            }
            let replacement_shard_index = replacement_root
                .as_ref()
                .map(|lowest_root| self.shard_index_for_root(lowest_root));

            if let Some(lowest_shard_index) = replacement_shard_index
                && lowest_shard_index != insert_shard_index
            {
                let (mut first, mut second) =
                    self.write_two_shards_ordered(insert_shard_index, lowest_shard_index);
                let (insert_shard, lowest_shard) = (&mut first, &mut second);

                if insert_shard.recently_confirmed_root_exists(&block.qualified_root()) {
                    return PriorityActivationResult::RecentlyConfirmed;
                }

                if insert_shard.find_bucket(&root) == Some(bucket_id) {
                    return PriorityActivationResult::Duplicate;
                }

                let removed = lowest_shard.erase(replacement_root.as_ref().unwrap());
                self.publish_shard_handle_removal(replacement_root.as_ref().unwrap());
                if let Some(election) = &removed {
                    self.stats.write().unwrap().stopped(election);
                }
                ended = removed;

                match insert_shard.insert(AecInsertRequest::new_priority(block, priority), now) {
                    Ok(inserted) => {
                        if let Some(root) = inserted.inserted_root() {
                            self.publish_shard_handle(root, &first);
                        }
                        self.stats_inserted(&inserted);
                        Ok((inserted, true))
                    }
                    Err(err) => Err(priority_activation_error(err)),
                }
            } else {
                let mut shard = self.shards[insert_shard_index].write().unwrap();

                if shard.recently_confirmed_root_exists(&block.qualified_root()) {
                    return PriorityActivationResult::RecentlyConfirmed;
                }

                if shard.find_bucket(&root) == Some(bucket_id) {
                    return PriorityActivationResult::Duplicate;
                }

                let replaced = if let Some(lowest_root) = &replacement_root {
                    let removed = shard.erase(lowest_root);
                    self.publish_shard_handle_removal(lowest_root);
                    if let Some(election) = &removed {
                        self.stats.write().unwrap().stopped(election);
                    }
                    ended = removed;
                    true
                } else {
                    false
                };

                match shard.insert(AecInsertRequest::new_priority(block, priority), now) {
                    Ok(inserted) => {
                        if let Some(root) = inserted.inserted_root() {
                            self.publish_shard_handle(root, &shard);
                        }
                        self.stats_inserted(&inserted);
                        Ok((inserted, replaced))
                    }
                    Err(err) => Err(priority_activation_error(err)),
                }
            }
        };

        let (inserted, replaced) = match inserted {
            Ok(value) => value,
            Err(result) => return result,
        };

        if let Some(election) = ended {
            self.router.write().unwrap().disconnect_election(&election);
            self.notify(AecFact::ElectionEnded(election));
        }

        if let InsertResult::Inserted { hash, root, .. } = inserted {
            self.router.write().unwrap().connect(hash, root.clone());
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
                let mut shard = self.shard(&root).write().unwrap();
                let change = shard.apply_fork_result(&root, &handle, fork, result);
                if matches!(
                    change,
                    ForkChange::Added { .. } | ForkChange::Replaced { .. }
                ) {
                    self.stats.write().unwrap().conflicts += 1;
                }
                change
            }
        };

        match change {
            ForkChange::Added { added_hash } => {
                self.router
                    .write()
                    .unwrap()
                    .connect(added_hash, root.clone());
                self.notify(AecFact::BlockAddedToElection(added_hash));
                true
            }
            ForkChange::Replaced {
                added_hash,
                removed,
            } => {
                let mut router = self.router.write().unwrap();
                router.disconnect(&removed.hash());
                router.connect(added_hash, root.clone());
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
        if let Some(handle) = self.election_handle_for_root(root) {
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
                self.vote_counter.count(args.vote.source);
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
            for _ in 0..counted_votes {
                self.vote_counter.count(args.vote.source);
            }
        }

        results
    }

    fn resolve_vote_result(&self, block_hash: BlockHash) -> ResolvedVoteResult {
        if let Some((root, _)) = self.routed_root(&block_hash)
            && let Some(handle) = self.published_handles.load().get(&root).cloned()
        {
            ResolvedVoteResult::Apply(handle)
        } else if self.was_recently_confirmed(&block_hash)
        {
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
            self.stats.write().unwrap().ticked += 1;
            ended
                .into_iter()
                .filter_map(|root| {
                    let ended = self.shard(&root).write().unwrap().erase(&root);
                    if ended.is_some() {
                        self.publish_shard_handle_removal(&root);
                    }
                    ended
                })
                .inspect(|election| self.stats.write().unwrap().stopped(election))
                .collect::<Vec<_>>()
        };

        self.finish_removed_elections(ended);
    }

    pub fn next_vote_in_bucket(
        &self,
        bucket_id: usize,
        vote_broadcast_interval: Duration,
        now: Timestamp,
    ) -> Option<(QualifiedRoot, VoteType, BlockHash)> {
        for shard_index in 0..self.shards.len() {
            let mut cursor = None;
            while let Some((next_cursor, root, handle)) =
                self.next_bucket_handle(shard_index, bucket_id, cursor.as_ref())
            {
                cursor = Some(next_cursor);
                let election = handle.lock();
                if election.can_vote(vote_broadcast_interval, now) {
                    return Some((root, election.vote_type(), election.winner().hash()));
                }
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
        let mut snapshots = Vec::new();
        self.for_each_election_snapshot(|election| {
            snapshots.push(election);
            true
        });
        snapshots
    }

    pub fn active_election_snapshots(&self) -> Vec<Election> {
        let mut snapshots = Vec::new();
        self.for_each_active_election(|election| {
            snapshots.push(election);
            true
        });
        snapshots
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
            let mut ended = Vec::new();
            for shard in &self.shards {
                let shard_ended = shard.write().unwrap().take_ended_elections();
                for election in &shard_ended {
                    self.publish_shard_handle_removal(election.qualified_root());
                }
                ended.extend(shard_ended);
            }
            if !ended.is_empty() {
                let mut stats = self.stats.write().unwrap();
                for election in &ended {
                    stats.stopped(election);
                }
            }
            ended
        };

        self.finish_removed_elections(ended);
    }

    pub fn erase(&self, root: &QualifiedRoot) -> bool {
        let ended = {
            let ended = self.shard(root).write().unwrap().erase(root);
            if ended.is_some() {
                self.publish_shard_handle_removal(root);
            }
            if let Some(election) = &ended {
                self.stats.write().unwrap().stopped(election);
            }
            ended
        };

        if let Some(election) = ended {
            self.finish_removed_elections([election]);
            true
        } else {
            false
        }
    }

    pub fn erase_lowest_prio_election(&self, bucket_id: usize) {
        let lowest = self.lowest_priority(bucket_id).map(|(root, _)| root);
        if let Some(election) = lowest.and_then(|root| {
            let election = self.shard(&root).write().unwrap().erase(&root);
            if election.is_some() {
                self.publish_shard_handle_removal(&root);
            }
            if let Some(election) = &election {
                self.stats.write().unwrap().stopped(election);
            }
            election
        }) {
            self.finish_removed_elections([election]);
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

        if !confirmed_results.is_empty() {
            let mut stats = self.stats.write().unwrap();
            for (_, election) in &confirmed_results {
                stats.block_confirmations[election.confirmation_type as usize] += 1;
            }
        }
        for (block, election) in confirmed_results {
            self.notify(AecFact::BlockConfirmed(block, election));
        }
    }

    pub fn remove_recently_confirmed(&self, block_hash: &BlockHash) {
        for shard in &self.shards {
            shard.write().unwrap().remove_recently_confirmed(block_hash);
        }
    }

    pub fn set_cooldown(&self, cool_down: bool, reason: AecCooldownReason) {
        let recovered = {
            self.lifecycle
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
        for shard in &self.shards {
            shard.write().unwrap().cancel_all();
        }
    }

    pub fn clear_recently_confirmed(&self) {
        for shard in &self.shards {
            shard.write().unwrap().clear_recently_confirmed();
        }
    }

    pub fn stop(&self) {
        let _ = self.observer.write().unwrap().take();
        self.router.write().unwrap().clear();
        let mut lifecycle = self.lifecycle.write().unwrap();
        lifecycle.stopped = true;
        for shard in &self.shards {
            let _ = shard.write().unwrap().stop();
        }
        self.published_handles.store(Arc::new(FxHashMap::default()));
    }

    pub fn force_confirm(&self, block_hash: &BlockHash, now: Timestamp) {
        let handle = self
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
            let mut shard = self.shard(election.qualified_root()).write().unwrap();
            shard.put_recently_confirmed(election.qualified_root().clone(), election.winner().hash());
            if shard.erase_with_known_election(election.qualified_root(), &election) {
                self.publish_shard_handle_removal(election.qualified_root());
                shard.record_cleanup_stopped(&election);
                Some(election)
            } else {
                None
            }
        };

        if let Some(election) = ended {
            self.finish_removed_elections([election]);
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
        let (root, _) = self.routed_root(block_hash)?;
        self.published_handles.load().get(&root).cloned()
    }

    fn election_handle_for_root(&self, root: &QualifiedRoot) -> Option<ElectionHandle> {
        self.published_handles.load().get(root).cloned()
    }

    fn next_bucket_handle(
        &self,
        shard_index: usize,
        bucket_id: usize,
        after: Option<&BucketCursor>,
    ) -> Option<(BucketCursor, QualifiedRoot, ElectionHandle)> {
        let shard = self.shards[shard_index].read().unwrap();
        shard
            .next_bucket(bucket_id, after)
            .map(|(cursor, (root, handle))| (cursor, root, handle))
    }

    fn for_each_round_robin_handle(
        &self,
        mut f: impl FnMut(usize, QualifiedRoot, ElectionHandle) -> bool,
    ) {
        for shard_index in 0..self.shards.len() {
            let mut cursors = vec![None; bucket_count()];
            let mut next_bucket = bucket_count().saturating_sub(1);

            while let Some((bucket_id, cursor, root, handle)) =
                self.next_round_robin_handle(shard_index, &cursors, next_bucket)
            {
                cursors[bucket_id] = Some(cursor);
                next_bucket = bucket_id.checked_sub(1).unwrap_or(bucket_count() - 1);
                if !f(bucket_id, root, handle) {
                    return;
                }
            }
        }
    }

    pub(crate) fn for_each_active_election(&self, mut f: impl FnMut(Election) -> bool) {
        self.for_each_election_snapshot(|election| {
            if election.state() == ElectionState::Active {
                f(election)
            } else {
                true
            }
        });
    }

    fn for_each_election_snapshot(&self, mut f: impl FnMut(Election) -> bool) {
        for shard in &self.shards {
            let shard = shard.read().unwrap();
            for election in shard.iter_round_robin() {
                if !f(election) {
                    return;
                }
            }
        }
    }

    fn next_round_robin_handle(
        &self,
        shard_index: usize,
        cursors: &[Option<BucketCursor>],
        start_bucket: usize,
    ) -> Option<(usize, BucketCursor, QualifiedRoot, ElectionHandle)> {
        for offset in 0..bucket_count() {
            let bucket_id = (start_bucket + bucket_count() - offset) % bucket_count();
            if let Some((cursor, root, handle)) =
                self.next_bucket_handle(shard_index, bucket_id, cursors[bucket_id].as_ref())
            {
                return Some((bucket_id, cursor, root, handle));
            }
        }
        None
    }

    fn routed_root(&self, block_hash: &BlockHash) -> Option<(QualifiedRoot, usize)> {
        let root = self
            .router
            .read()
            .unwrap()
            .qualified_root(block_hash)
            .cloned()?;
        let shard_index = self.shard_index_for_root(&root);
        Some((root, shard_index))
    }

    fn shard(&self, root: &QualifiedRoot) -> &RwLock<AecShardState> {
        &self.shards[self.shard_index_for_root(root)]
    }

    fn shard_index_for_root(&self, root: &QualifiedRoot) -> usize {
        root.to_bytes()[QualifiedRoot::SERIALIZED_SIZE - 1] as usize % self.shards.len()
    }

    fn publish_shard_handle(&self, root: &QualifiedRoot, shard: &AecShardState) {
        let Some(handle) = shard.election_handle_for_root(root) else {
            return;
        };
        let current = self.published_handles.load();
        let mut updated = (*current).clone();
        updated.insert(root.clone(), handle);
        self.published_handles.store(Arc::new(updated));
    }

    fn publish_shard_handle_removal(&self, root: &QualifiedRoot) {
        let current = self.published_handles.load();
        let mut updated = (*current).clone();
        updated.remove(root);
        self.published_handles.store(Arc::new(updated));
    }

    fn finish_removed_elections(&self, elections: impl IntoIterator<Item = Election>) {
        let elections: Vec<_> = elections.into_iter().collect();
        if elections.is_empty() {
            return;
        }

        {
            let mut router = self.router.write().unwrap();
            for election in &elections {
                router.disconnect_election(election);
            }
        }

        for election in elections {
            self.notify(AecFact::ElectionEnded(election));
        }
    }

    fn write_two_shards_ordered(
        &self,
        first_index: usize,
        second_index: usize,
    ) -> (
        RwLockWriteGuard<'_, AecShardState>,
        RwLockWriteGuard<'_, AecShardState>,
    ) {
        debug_assert_ne!(first_index, second_index);
        let (low, high) = if first_index < second_index {
            (first_index, second_index)
        } else {
            (second_index, first_index)
        };
        let low_guard = self.shards[low].write().unwrap();
        let high_guard = self.shards[high].write().unwrap();
        if first_index < second_index {
            (low_guard, high_guard)
        } else {
            (high_guard, low_guard)
        }
    }
}

struct AtomicArc<T> {
    ptr: AtomicPtr<T>,
}

impl<T> AtomicArc<T> {
    fn new(value: Arc<T>) -> Self {
        Self {
            ptr: AtomicPtr::new(Arc::into_raw(value) as *mut T),
        }
    }

    fn load(&self) -> Arc<T> {
        let ptr = self.ptr.load(Ordering::Acquire);
        unsafe {
            Arc::increment_strong_count(ptr);
            Arc::from_raw(ptr)
        }
    }

    fn store(&self, value: Arc<T>) {
        let new_ptr = Arc::into_raw(value) as *mut T;
        let old_ptr = self.ptr.swap(new_ptr, Ordering::AcqRel);
        unsafe {
            drop(Arc::from_raw(old_ptr));
        }
    }
}

impl<T> Drop for AtomicArc<T> {
    fn drop(&mut self) {
        let ptr = self.ptr.load(Ordering::Relaxed);
        unsafe {
            drop(Arc::from_raw(ptr));
        }
    }
}

impl StatsSource for AecService {
    fn collect_stats(&self, result: &mut StatsCollection) {
        let lifecycle = self.lifecycle.read().unwrap();
        lifecycle.cooldown.collect_stats(result);
        self.stats.read().unwrap().collect_stats(result);
        for shard in &self.shards {
            let mut shard_stats = StatsCollection::new();
            shard.read().unwrap().cleanup_stats.collect_stats(&mut shard_stats);
            merge_stats(result, &shard_stats);
        }
        self.vote_counter.collect_stats(result);
    }
}

impl ContainerInfoProvider for AecService {
    fn container_info(&self) -> ContainerInfo {
        let recently_confirmed_count: usize = self
            .shards
            .iter()
            .map(|shard| shard.read().unwrap().recently_confirmed_len())
            .sum();
        ContainerInfo::builder()
            .leaf("roots", self.len(), RootContainer::ELEMENT_SIZE)
            .leaf(
                "normal",
                self.count_by_behavior(ElectionBehavior::Priority),
                0,
            )
            .leaf(
                "hinted".to_string(),
                self.count_by_behavior(ElectionBehavior::Hinted),
                0,
            )
            .leaf(
                "optimistic".to_string(),
                self.count_by_behavior(ElectionBehavior::Optimistic),
                0,
            )
            .node(
                "recently_confirmed",
                [(
                    "confirmed",
                    recently_confirmed_count,
                    std::mem::size_of::<BlockHash>() * 3 + std::mem::size_of::<QualifiedRoot>(),
                )]
                .into(),
            )
            .node("vote_router", self.router.read().unwrap().container_info())
            .finish()
    }
}

enum ResolvedVoteResult {
    Apply(ElectionHandle),
    Resolved(Result<(), VoteError>),
}

impl InsertResult {
    fn inserted_root(&self) -> Option<&QualifiedRoot> {
        match self {
            InsertResult::Inserted { root, .. } => Some(root),
            InsertResult::Upgraded { .. } => None,
        }
    }
}

fn merge_stats(target: &mut StatsCollection, source: &StatsCollection) {
    for (key, value) in source.iter() {
        let total = target.get_dir(key.stat, key.detail, key.dir) + value;
        target.insert_dir(key.stat, key.detail, key.dir, total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        consensus::election_schedulers::priority::bucket_index,
        consensus::{AecInsertRequest, ReceivedVote},
        representatives::QuorumSpecs,
    };
    use rsnano_ledger::RepWeights;
    use rsnano_types::{
        BlockPriority, PrivateKey, SavedBlock, UnixMillisTimestamp, Vote, VoteSource,
    };
    use rsnano_utils::{container_info::ContainerInfoEntry, stats::StatsCollection};
    use rsnano_utils::sync::backpressure_channel::channel;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Instant,
    };

    fn block_in_shard(aec: &AecService, shard_index: usize) -> SavedBlock {
        (1..256)
            .map(SavedBlock::new_test_instance_with_key)
            .find(|block| aec.shard_index_for_root(&block.qualified_root()) == shard_index)
            .unwrap()
    }

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

        let locked_handle = aec.election_handle_for_block(&block_a.hash()).unwrap();
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
                container_write_available = true;
                break;
            }
            if aec.shard(&block_a.qualified_root()).try_write().is_ok() {
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
    fn confirmed_cleanup_does_not_wait_for_global_stats_writer() {
        let aec = Arc::new(AecService::new_null());
        let block_a = block_in_shard(&aec, 0);
        let block_b = block_in_shard(&aec, 1);
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

        let rep_key = PrivateKey::from(1);
        let mut rep_weights = RepWeights::default();
        rep_weights.put(rep_key.public_key(), Amount::MAX);
        let vote: ReceivedVote = ReceivedVote::new(
            Vote::new_final(&rep_key, vec![block_b.hash()]).into(),
            VoteSource::Live,
            None,
        );

        let stats_guard = aec.stats.write().unwrap();
        let aec_for_thread = Arc::clone(&aec);
        let worker = thread::spawn(move || {
            aec_for_thread.apply_vote(ApplyVoteArgs {
                vote: &vote.into(),
                rep_weights: &rep_weights,
                quorum_specs: &QuorumSpecs::new_test_instance(),
                now,
            })
        });

        let start = Instant::now();
        while !worker.is_finished() && start.elapsed() < Duration::from_secs(1) {
            thread::yield_now();
        }

        assert!(worker.is_finished());

        drop(stats_guard);
        let results = worker.join().unwrap();
        assert_eq!(results.get(&block_b.hash()), Some(&Ok(())));
        assert!(!aec.is_active_root(&block_b.qualified_root()));
        assert!(aec.was_recently_confirmed(&block_b.hash()));
    }

    #[test]
    fn apply_vote_on_same_shard_does_not_wait_for_unrelated_shard_write() {
        let aec = Arc::new(AecService::new_null());
        let block_a = block_in_shard(&aec, 0);
        let block_b = (1..256)
            .map(SavedBlock::new_test_instance_with_key)
            .find(|block| {
                aec.shard_index_for_root(&block.qualified_root())
                    == aec.shard_index_for_root(&block_a.qualified_root())
                    && block.qualified_root() != block_a.qualified_root()
            })
            .unwrap();
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

        let rep_key = PrivateKey::from(1);
        let mut rep_weights = RepWeights::default();
        rep_weights.put(rep_key.public_key(), Amount::MAX);
        let vote: ReceivedVote = ReceivedVote::new(
            Vote::new(
                &rep_key,
                UnixMillisTimestamp::ZERO,
                0,
                vec![block_b.hash()],
            )
            .into(),
            VoteSource::Live,
            None,
        );

        let shard_guard = aec.shard(&block_a.qualified_root()).write().unwrap();
        let aec_for_thread = Arc::clone(&aec);
        let worker = thread::spawn(move || {
            aec_for_thread.apply_vote(ApplyVoteArgs {
                vote: &vote.into(),
                rep_weights: &rep_weights,
                quorum_specs: &QuorumSpecs::new_test_instance(),
                now,
            })
        });

        let start = Instant::now();
        while !worker.is_finished() && start.elapsed() < Duration::from_secs(1) {
            thread::yield_now();
        }

        assert!(worker.is_finished());

        drop(shard_guard);
        let results = worker.join().unwrap();
        assert_eq!(results.get(&block_b.hash()), Some(&Ok(())));
        assert!(aec.is_active_root(&block_b.qualified_root()));
        assert!(!aec.was_recently_confirmed(&block_b.hash()));
    }

    #[test]
    fn counted_vote_does_not_wait_for_global_write_to_update_stats() {
        let aec = Arc::new(AecService::new_null());
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
        let quorum_specs = QuorumSpecs::new_test_instance();
        let vote: ReceivedVote = ReceivedVote::new(
            Vote::new(
                &rep_key,
                UnixMillisTimestamp::ZERO,
                0,
                vec![block.hash()],
            )
            .into(),
            VoteSource::Live,
            None,
        );

        let read_guard = aec.lifecycle.read().unwrap();
        let aec_for_thread = Arc::clone(&aec);
        let worker = thread::spawn(move || {
            aec_for_thread.apply_vote(ApplyVoteArgs {
                vote: &vote.into(),
                rep_weights: &rep_weights,
                quorum_specs: &quorum_specs,
                now,
            })
        });

        let start = Instant::now();
        while !worker.is_finished() && start.elapsed() < Duration::from_secs(1) {
            thread::yield_now();
        }
        assert!(worker.is_finished());

        let results = worker.join().unwrap();
        drop(read_guard);

        assert_eq!(results.get(&block.hash()), Some(&Ok(())));
        assert!(aec.is_active_root(&block.qualified_root()));
        assert!(!aec.was_recently_confirmed(&block.hash()));

        let mut stats = StatsCollection::new();
        aec.collect_stats(&mut stats);
        assert_eq!(stats.get("election", "vote"), 1);
        assert_eq!(stats.get("election_vote", "live"), 1);
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
    fn insert_places_election_in_one_deterministic_shard() {
        let aec = AecService::new_null();
        let block = SavedBlock::new_test_instance();
        let shard_index = aec.shard_index_for_root(&block.qualified_root());

        aec.insert(
            AecInsertRequest::new_priority(block.clone(), BlockPriority::new_test_instance()),
            Timestamp::new_test_instance(),
        )
        .unwrap();

        for (index, shard) in aec.shards.iter().enumerate() {
            let len = shard.read().unwrap().len();
            if index == shard_index {
                assert_eq!(len, 1);
            } else {
                assert_eq!(len, 0);
            }
        }

        assert_eq!(
            aec.shard_index_for_root(&block.qualified_root()),
            shard_index
        );
    }

    #[test]
    fn block_lookup_uses_service_owned_router_with_direct_published_handles() {
        let aec = AecService::new_null();
        let block = SavedBlock::new_test_instance();
        let hash = block.hash();
        let root = block.qualified_root();

        aec.insert(
            AecInsertRequest::new_priority(block.clone(), BlockPriority::new_test_instance()),
            Timestamp::new_test_instance(),
        )
        .unwrap();

        assert_eq!(
            aec.router.read().unwrap().qualified_root(&hash),
            Some(&root)
        );
        assert_eq!(
            aec.election_for_block(&hash).unwrap().qualified_root(),
            &root
        );
    }

    #[test]
    fn root_lookup_helpers_do_not_wait_for_same_shard_write_lock() {
        let aec = Arc::new(AecService::new_null());
        let block_a = block_in_shard(&aec, 0);
        let block_b = (1..256)
            .map(SavedBlock::new_test_instance_with_key)
            .find(|block| {
                aec.shard_index_for_root(&block.qualified_root())
                    == aec.shard_index_for_root(&block_a.qualified_root())
                    && block.qualified_root() != block_a.qualified_root()
            })
            .unwrap();
        let now = Timestamp::new_test_instance();

        aec.insert(
            AecInsertRequest::new_priority(block_a, BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();
        aec.insert(
            AecInsertRequest::new_priority(block_b.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        let shard_guard = aec.shard(&block_b.qualified_root()).write().unwrap();
        let aec_for_thread = Arc::clone(&aec);
        let lookup_root = block_b.qualified_root();
        let worker = thread::spawn(move || {
            let election = aec_for_thread.election_for_root(&lookup_root).unwrap();
            assert_eq!(election.qualified_root(), &lookup_root);
            assert!(aec_for_thread.is_active_root(&lookup_root));
        });

        let deadline = Instant::now() + Duration::from_secs(1);
        while !worker.is_finished() && Instant::now() < deadline {
            thread::yield_now();
        }

        assert!(worker.is_finished());

        drop(shard_guard);
        worker.join().unwrap();
    }

    #[test]
    fn activate_priority_replaces_lowest_election_across_shards() {
        let aec = AecService::new_null();
        let block_a = SavedBlock::new_test_instance_with_key(1);
        let block_b = (2..64)
            .map(SavedBlock::new_test_instance_with_key)
            .find(|block| {
                aec.shard_index_for_root(&block.qualified_root())
                    != aec.shard_index_for_root(&block_a.qualified_root())
            })
            .unwrap();
        let low_priority = BlockPriority::new(Amount::nano(1), TimePriority::new(1));
        let high_priority = BlockPriority::new(Amount::nano(1), TimePriority::new(2));
        let bucket_id = bucket_index(ElectionBehavior::Priority, low_priority.balance);
        let now = Timestamp::new_test_instance();

        assert_eq!(
            aec.activate_priority(bucket_id, 1, block_a.clone(), low_priority, now),
            PriorityActivationResult::Activated
        );
        assert_eq!(
            aec.activate_priority(bucket_id, 1, block_b.clone(), high_priority, now),
            PriorityActivationResult::ActivatedWithReplacement
        );

        assert!(!aec.is_active_root(&block_a.qualified_root()));
        assert!(aec.is_active_root(&block_b.qualified_root()));
        assert!(!aec.is_active_hash(&block_a.hash()));
        assert!(aec.is_active_hash(&block_b.hash()));
    }

    #[test]
    fn election_snapshots_collect_across_shards() {
        let aec = AecService::new_null();
        let block_a = (1..64)
            .map(SavedBlock::new_test_instance_with_key)
            .find(|block| aec.shard_index_for_root(&block.qualified_root()) == 0)
            .unwrap();
        let block_b = (1..64)
            .map(SavedBlock::new_test_instance_with_key)
            .find(|block| aec.shard_index_for_root(&block.qualified_root()) > 0)
            .unwrap();
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

        let seen: Vec<_> = aec
            .election_snapshots()
            .into_iter()
            .map(|election| election.qualified_root().clone())
            .collect();

        assert!(seen.contains(&block_a.qualified_root()));
        assert!(seen.contains(&block_b.qualified_root()));
    }

    #[test]
    fn container_info_aggregates_sharded_root_count() {
        let aec = AecService::new_null();
        let block_a = SavedBlock::new_test_instance_with_key(1);
        let block_b = (2..64)
            .map(SavedBlock::new_test_instance_with_key)
            .find(|block| {
                aec.shard_index_for_root(&block.qualified_root())
                    != aec.shard_index_for_root(&block_a.qualified_root())
            })
            .unwrap();
        let now = Timestamp::new_test_instance();

        aec.insert(
            AecInsertRequest::new_priority(block_a, BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();
        aec.insert(
            AecInsertRequest::new_priority(block_b, BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        let ContainerInfoEntry::Leaf(roots) = &aec.container_info()[0] else {
            panic!("expected roots leaf");
        };
        assert_eq!(roots.info.count, 2);
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
        let block_a = block_in_shard(&aec, 0);
        let block_b = block_in_shard(&aec, 1);
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

        let locked_handle = aec.election_handle_for_block(&block_a.hash()).unwrap();
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
            if aec.shard(&block_a.qualified_root()).try_write().is_ok() {
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
    fn transition_active_on_other_shard_is_not_blocked_by_unrelated_shard_write_lock() {
        let aec = Arc::new(AecService::new_null());
        let block_a = block_in_shard(&aec, 0);
        let block_b = block_in_shard(&aec, 1);
        let now = Timestamp::new_test_instance();
        assert_ne!(
            aec.shard_index_for_root(&block_a.qualified_root()),
            aec.shard_index_for_root(&block_b.qualified_root())
        );

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

        let unrelated_shard_guard = aec.shard(&block_a.qualified_root()).write().unwrap();
        let started = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let aec_for_thread = Arc::clone(&aec);
        let started_clone = Arc::clone(&started);
        let finished_clone = Arc::clone(&finished);
        let block_b_hash = block_b.hash();

        let worker = thread::spawn(move || {
            started_clone.store(true, Ordering::Release);
            assert!(aec_for_thread.transition_active(&block_b_hash));
            finished_clone.store(true, Ordering::Release);
        });

        while !started.load(Ordering::Acquire) {
            thread::yield_now();
        }

        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if finished.load(Ordering::Acquire) {
                break;
            }
            thread::yield_now();
        }

        let finished_before_release = finished.load(Ordering::Acquire);
        drop(unrelated_shard_guard);
        worker.join().unwrap();
        assert!(finished_before_release);
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
            if aec.shard(&block.qualified_root()).try_write().is_ok() {
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
    fn election_snapshot_scan_releases_one_shard_before_waiting_on_the_next() {
        let aec = Arc::new(AecService::new_null());
        let block_a = block_in_shard(&aec, 0);
        let block_b = block_in_shard(&aec, 1);
        let now = Timestamp::new_test_instance();
        assert_ne!(
            aec.shard_index_for_root(&block_a.qualified_root()),
            aec.shard_index_for_root(&block_b.qualified_root())
        );

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

        let blocked_shard_guard = aec.shard(&block_b.qualified_root()).write().unwrap();
        let started = Arc::new(AtomicBool::new(false));
        let saw_first_shard = Arc::new(AtomicBool::new(false));
        let aec_for_thread = Arc::clone(&aec);
        let started_clone = Arc::clone(&started);
        let saw_first_shard_clone = Arc::clone(&saw_first_shard);
        let first_root = block_a.qualified_root();

        let worker = thread::spawn(move || {
            started_clone.store(true, Ordering::Release);
            aec_for_thread.for_each_election_snapshot(|election| {
                if election.qualified_root() == &first_root {
                    saw_first_shard_clone.store(true, Ordering::Release);
                }
                true
            });
        });

        while !started.load(Ordering::Acquire) {
            thread::yield_now();
        }
        let saw_deadline = Instant::now() + Duration::from_secs(1);
        while !saw_first_shard.load(Ordering::Acquire) && Instant::now() < saw_deadline {
            thread::yield_now();
        }
        let saw_first_shard_before_release = saw_first_shard.load(Ordering::Acquire);

        let mut first_shard_released = false;
        let write_deadline = Instant::now() + Duration::from_secs(1);
        while saw_first_shard_before_release && Instant::now() < write_deadline {
            if aec.shard(&block_a.qualified_root()).try_write().is_ok() {
                first_shard_released = true;
                break;
            }
            thread::yield_now();
        }

        drop(blocked_shard_guard);
        worker.join().unwrap();
        assert!(saw_first_shard_before_release);
        assert!(first_shard_released);
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
