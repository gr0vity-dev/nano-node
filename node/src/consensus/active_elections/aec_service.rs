use std::{
    cmp::Ordering as CmpOrdering,
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, RwLock,
        atomic::{AtomicPtr, Ordering},
    },
    time::Duration,
};

use rsnano_nullable_clock::Timestamp;
use rsnano_types::{
    Account, Amount, Block, BlockHash, BlockPriority, PublicKey, QualifiedRoot, SavedBlock,
    TimePriority, VoteError,
};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{StatsCollection, StatsSource},
    sync::backpressure_channel::Sender,
};

use super::{
    ActiveElectionsConfig, ActiveElectionsInfo, AecCooldownReason, AecFact, AecInsertError,
    AecInsertRequest, ApplyVoteArgs, RootContainer,
    active_elections_container::{ForkChange, InsertResult},
    apply_vote_helper::ApplyVoteHelper,
    cooldown_controller::{CooldownController, CooldownResult},
    root_container::ElectionHandle,
    stats::{AecStats, VoteCounter},
    vote_router::VoteRouter,
};
use crate::consensus::election::{
    AddForkResult, ConfirmationType, ConfirmedElection, Election, ElectionBehavior, ElectionState,
    VoteType,
};
use crate::consensus::election_schedulers::priority::{bucket_count, bucket_index};
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
    base_latency: Duration,
    router: RwLock<VoteRouter>,
    published: AtomicArc<PublishedElectionMap>,
    recently_confirmed: AtomicArc<RecentlyConfirmedState>,
    lifecycle: RwLock<AecLifecycleState>,
    stats: RwLock<AecStats>,
    cleanup_stats: RwLock<AecStats>,
    vote_counter: VoteCounter,
    observer: RwLock<Option<Sender<AecFact>>>,
}

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

type PublishedElectionMap = FxHashMap<QualifiedRoot, PublishedElection>;

#[derive(Clone)]
struct PublishedElection {
    handle: ElectionHandle,
    priority: BlockPriority,
}

#[derive(Clone, Default)]
struct RecentlyConfirmedState {
    by_root: FxHashMap<QualifiedRoot, BlockHash>,
    by_hash: FxHashMap<BlockHash, QualifiedRoot>,
    ordered_hashes: VecDeque<BlockHash>,
    max_len: usize,
}

impl RecentlyConfirmedState {
    fn new(max_len: usize) -> Self {
        Self {
            max_len,
            ..Default::default()
        }
    }

    fn root_exists(&self, root: &QualifiedRoot) -> bool {
        self.by_root.contains_key(root)
    }

    fn hash_exists(&self, block_hash: &BlockHash) -> bool {
        self.by_hash.contains_key(block_hash)
    }

    fn put(&mut self, root: QualifiedRoot, hash: BlockHash) {
        if let Some(previous_hash) = self.by_root.insert(root.clone(), hash) {
            self.by_hash.remove(&previous_hash);
            self.ordered_hashes
                .retain(|current| current != &previous_hash);
        }

        if let Some(previous_root) = self.by_hash.insert(hash, root.clone()) {
            self.by_root.remove(&previous_root);
            self.ordered_hashes.retain(|current| current != &hash);
        }

        self.ordered_hashes.push_back(hash);

        while self.by_hash.len() > self.max_len {
            let Some(oldest_hash) = self.ordered_hashes.pop_front() else {
                break;
            };
            let Some(oldest_root) = self.by_hash.remove(&oldest_hash) else {
                continue;
            };
            self.by_root.remove(&oldest_root);
        }
    }

    fn erase(&mut self, block_hash: &BlockHash) {
        if let Some(root) = self.by_hash.remove(block_hash) {
            self.by_root.remove(&root);
            self.ordered_hashes.retain(|current| current != block_hash);
        }
    }

    fn clear(&mut self) {
        self.by_root.clear();
        self.by_hash.clear();
        self.ordered_hashes.clear();
    }

    fn len(&self) -> usize {
        self.by_hash.len()
    }
}

impl AecService {
    fn stats_inserted(&self, result: &InsertResult) {
        if let InsertResult::Inserted { behavior, .. } = result {
            self.stats.write().unwrap().started(*behavior);
        }
    }

    pub fn new(config: ActiveElectionsConfig, base_latency: Duration) -> Self {
        let confirmation_cache = config.confirmation_cache;
        Self {
            base_latency,
            router: RwLock::new(VoteRouter::default()),
            published: AtomicArc::new(Arc::new(FxHashMap::default())),
            recently_confirmed: AtomicArc::new(Arc::new(RecentlyConfirmedState::new(
                confirmation_cache,
            ))),
            lifecycle: RwLock::new(AecLifecycleState::new(config)),
            stats: RwLock::new(AecStats::default()),
            cleanup_stats: RwLock::new(AecStats::default()),
            vote_counter: VoteCounter::default(),
            observer: RwLock::new(None),
        }
    }

    pub fn new_null() -> Self {
        Self {
            base_latency: Duration::from_secs(0),
            router: RwLock::new(VoteRouter::default()),
            published: AtomicArc::new(Arc::new(FxHashMap::default())),
            recently_confirmed: AtomicArc::new(Arc::new(RecentlyConfirmedState::new(
                ActiveElectionsConfig::default().confirmation_cache,
            ))),
            lifecycle: RwLock::new(AecLifecycleState::new(ActiveElectionsConfig::default())),
            stats: RwLock::new(AecStats::default()),
            cleanup_stats: RwLock::new(AecStats::default()),
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
        self.published.load().len()
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
        self.recently_confirmed.load().hash_exists(block_hash)
    }

    pub fn count_by_behavior(&self, behavior: ElectionBehavior) -> usize {
        self.published
            .load()
            .values()
            .filter(|entry| entry.handle.behavior() == behavior)
            .count()
    }

    pub fn bucket_len(&self, bucket_id: usize) -> usize {
        self.published
            .load()
            .iter()
            .filter(|(root, entry)| self.bucket_of_entry(root, entry) == bucket_id)
            .count()
    }

    pub fn find_bucket(&self, root: &QualifiedRoot) -> Option<usize> {
        self.published
            .load()
            .get(root)
            .map(|entry| self.bucket_of_entry(root, entry))
    }

    pub fn lowest_priority(&self, bucket_id: usize) -> Option<(QualifiedRoot, TimePriority)> {
        self.published
            .load()
            .iter()
            .filter(|(root, entry)| self.bucket_of_entry(root, entry) == bucket_id)
            .min_by(|(left_root, left_entry), (right_root, right_entry)| {
                self.compare_lowest_priority(left_root, left_entry, right_root, right_entry)
            })
            .map(|(root, entry)| (root.clone(), entry.priority.time))
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
        let inserted = {
            let lifecycle = self.lifecycle.read().unwrap();
            lifecycle.ensure_not_stopped()?;
            if self
                .recently_confirmed
                .load()
                .root_exists(&request.block.qualified_root())
            {
                return Err(AecInsertError::RecentlyConfirmed);
            }
            let inserted = self.insert_published(request, now)?;
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
        let replacement_root = if self.bucket_len(bucket_id) >= reserved_elections {
            self.lowest_priority(bucket_id).map(|(root, _)| root)
        } else {
            None
        };
        let (inserted, ended) = {
            let lifecycle = self.lifecycle.read().unwrap();
            if let Err(err) = lifecycle.ensure_not_stopped() {
                return priority_activation_error(err);
            }
            if self
                .recently_confirmed
                .load()
                .root_exists(&block.qualified_root())
            {
                return PriorityActivationResult::RecentlyConfirmed;
            }

            if self.find_bucket(&root) == Some(bucket_id) {
                return PriorityActivationResult::Duplicate;
            }

            match self.activate_priority_published(block, priority, now, replacement_root.clone()) {
                Ok(value) => value,
                Err(err) => return priority_activation_error(err),
            }
        };

        if let Some(election) = ended {
            self.stats.write().unwrap().stopped(&election);
            self.router.write().unwrap().disconnect_election(&election);
            self.notify(AecFact::ElectionEnded(election));
        }

        if let InsertResult::Inserted { hash, root, .. } = inserted {
            self.router.write().unwrap().connect(hash, root.clone());
            self.notify(AecFact::ElectionStarted(hash, root));
        }

        if replacement_root.is_some() {
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
            result => self.apply_fork_result(&root, &handle, fork, result),
        };

        if matches!(
            change,
            ForkChange::Added { .. } | ForkChange::Replaced { .. }
        ) {
            self.stats.write().unwrap().conflicts += 1;
        }

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
        if let Some(root) = self
            .router
            .read()
            .unwrap()
            .qualified_root(&block_hash)
            .cloned()
            && let Some(handle) = self
                .published
                .load()
                .get(&root)
                .map(|entry| entry.handle.clone())
        {
            ResolvedVoteResult::Apply(handle)
        } else if self.was_recently_confirmed(&block_hash) {
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
                .filter_map(|root| self.remove_published_election(&root))
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
        for (root, handle) in self.bucket_handles(bucket_id) {
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
            for root in self.ended_roots() {
                if let Some(election) = self.remove_published_election(&root) {
                    ended.push(election);
                }
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
            let ended = self.remove_published_election(root);
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
        if let Some(election) = self
            .lowest_priority(bucket_id)
            .map(|(root, _)| root)
            .and_then(|root| self.remove_published_election(&root))
        {
            self.stats.write().unwrap().stopped(&election);
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
        self.update_recently_confirmed(|recently_confirmed| recently_confirmed.erase(block_hash));
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
        for entry in self.published.load().values() {
            entry.handle.lock().cancel();
        }
    }

    pub fn clear_recently_confirmed(&self) {
        self.update_recently_confirmed(RecentlyConfirmedState::clear);
    }

    pub fn stop(&self) {
        let _ = self.observer.write().unwrap().take();
        self.router.write().unwrap().clear();
        let mut lifecycle = self.lifecycle.write().unwrap();
        lifecycle.stopped = true;
        self.published.store(Arc::new(FxHashMap::default()));
        self.recently_confirmed
            .store(Arc::new(RecentlyConfirmedState::new(0)));
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
            self.update_recently_confirmed(|recently_confirmed| {
                recently_confirmed.put(election.qualified_root().clone(), election.winner().hash());
            });

            if self.remove_known_published_election(&election) {
                self.cleanup_stats.write().unwrap().stopped(&election);
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
        let root = self
            .router
            .read()
            .unwrap()
            .qualified_root(block_hash)
            .cloned()?;
        self.election_handle_for_root(&root)
    }

    fn election_handle_for_root(&self, root: &QualifiedRoot) -> Option<ElectionHandle> {
        self.published
            .load()
            .get(root)
            .map(|entry| entry.handle.clone())
    }

    fn for_each_round_robin_handle(
        &self,
        mut f: impl FnMut(usize, QualifiedRoot, ElectionHandle) -> bool,
    ) {
        for (bucket_id, root, handle) in self.round_robin_handles() {
            if !f(bucket_id, root, handle) {
                return;
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
        for (_, _, handle) in self.round_robin_handles() {
            if !f(handle.snapshot()) {
                return;
            }
        }
    }

    fn update_recently_confirmed(&self, f: impl FnOnce(&mut RecentlyConfirmedState)) {
        let current = self.recently_confirmed.load();
        let mut updated = (*current).clone();
        f(&mut updated);
        self.recently_confirmed.store(Arc::new(updated));
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

    fn insert_published(
        &self,
        request: AecInsertRequest,
        now: Timestamp,
    ) -> Result<InsertResult, AecInsertError> {
        let root = request.block.qualified_root();

        if let Some(entry) = self.published.load().get(&root).cloned() {
            let previous_behavior = entry.handle.behavior();
            if request.behavior != ElectionBehavior::Priority {
                return Err(AecInsertError::Duplicate);
            }

            if entry
                .handle
                .lock()
                .maybe_upgrade_to(ElectionBehavior::Priority)
            {
                return Ok(InsertResult::Upgraded {
                    previous_behavior,
                    new_behavior: ElectionBehavior::Priority,
                });
            } else {
                return Err(AecInsertError::Duplicate);
            }
        }

        let hash = request.block.hash();
        let priority = request.priority;
        let handle = ElectionHandle::new(Election::new(
            request.block,
            request.behavior,
            self.base_latency,
            now,
        ));
        self.published.update(|current| {
            let mut updated = current.clone();
            updated.insert(
                root.clone(),
                PublishedElection {
                    handle: handle.clone(),
                    priority,
                },
            );
            updated
        });

        Ok(InsertResult::Inserted {
            hash,
            root,
            behavior: request.behavior,
        })
    }

    fn activate_priority_published(
        &self,
        block: SavedBlock,
        priority: BlockPriority,
        now: Timestamp,
        replacement_root: Option<QualifiedRoot>,
    ) -> Result<(InsertResult, Option<Election>), AecInsertError> {
        let root = block.qualified_root();
        let hash = block.hash();
        let mut upgraded_handle: Option<(ElectionHandle, ElectionBehavior)> = None;

        loop {
            let current = self.published.load();
            let existing = current.get(&root).cloned();
            let already_upgraded = existing.as_ref().is_some_and(|entry| {
                upgraded_handle
                    .as_ref()
                    .is_some_and(|(handle, _)| entry.handle.ptr_eq(handle))
                    && entry.handle.behavior() == ElectionBehavior::Priority
            });

            let inserted = if let Some(entry) = &existing {
                if !already_upgraded {
                    let previous_behavior = entry.handle.behavior();
                    if !entry
                        .handle
                        .lock()
                        .maybe_upgrade_to(ElectionBehavior::Priority)
                    {
                        return Err(AecInsertError::Duplicate);
                    }
                    upgraded_handle = Some((entry.handle.clone(), previous_behavior));
                    InsertResult::Upgraded {
                        previous_behavior,
                        new_behavior: ElectionBehavior::Priority,
                    }
                } else {
                    InsertResult::Upgraded {
                        previous_behavior: upgraded_handle.as_ref().unwrap().1,
                        new_behavior: ElectionBehavior::Priority,
                    }
                }
            } else {
                InsertResult::Inserted {
                    hash,
                    root: root.clone(),
                    behavior: ElectionBehavior::Priority,
                }
            };

            let replaced = replacement_root.as_ref().and_then(|candidate| {
                current
                    .get(candidate)
                    .map(|entry| (candidate.clone(), entry.clone()))
            });

            if existing.is_some() && replaced.is_none() {
                return Ok((inserted, None));
            }

            let handle = ElectionHandle::new(Election::new(
                block.clone(),
                ElectionBehavior::Priority,
                self.base_latency,
                now,
            ));

            let mut updated = (*current).clone();
            if let Some((candidate, _)) = &replaced {
                updated.remove(candidate);
            }
            if existing.is_none() {
                updated.insert(
                    root.clone(),
                    PublishedElection {
                        handle: handle.clone(),
                        priority,
                    },
                );
            }

            if self.published.compare_exchange(&current, Arc::new(updated)) {
                let ended = replaced.map(|(_, entry)| entry.handle.snapshot());
                return Ok((inserted, ended));
            }
        }
    }

    fn apply_fork_result(
        &self,
        root: &QualifiedRoot,
        handle: &ElectionHandle,
        fork: &Block,
        result: AddForkResult,
    ) -> ForkChange {
        let current = self.published.load();
        let Some(entry) = current.get(root) else {
            return ForkChange::Ignored;
        };
        if !entry.handle.ptr_eq(handle) {
            return ForkChange::Ignored;
        }

        match result {
            AddForkResult::Added => ForkChange::Added {
                added_hash: fork.hash(),
            },
            AddForkResult::Replaced(removed) => ForkChange::Replaced {
                added_hash: fork.hash(),
                removed: removed.into(),
            },
            AddForkResult::TallyTooLow => ForkChange::Discarded {
                discarded: fork.clone(),
            },
            AddForkResult::Duplicate | AddForkResult::ElectionEnded => ForkChange::Ignored,
        }
    }

    fn remove_published_election(&self, root: &QualifiedRoot) -> Option<Election> {
        self.remove_published_if(root, |_| true)
    }

    fn remove_known_published_election(&self, election: &Election) -> bool {
        self.remove_published_if(election.qualified_root(), |entry| {
            entry.handle.snapshot().winner().hash() == election.winner().hash()
        })
        .is_some()
    }

    fn remove_published_if(
        &self,
        root: &QualifiedRoot,
        predicate: impl Fn(&PublishedElection) -> bool,
    ) -> Option<Election> {
        loop {
            let current = self.published.load();
            let entry = current.get(root)?.clone();
            if !predicate(&entry) {
                return None;
            }
            let mut updated = (*current).clone();
            updated.remove(root)?;
            if self.published.compare_exchange(&current, Arc::new(updated)) {
                return Some(entry.handle.snapshot());
            }
        }
    }

    fn ended_roots(&self) -> Vec<QualifiedRoot> {
        self.published
            .load()
            .iter()
            .filter_map(|(root, entry)| {
                if entry.handle.lock().state().has_ended() {
                    Some(root.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    fn bucket_of_entry(&self, _root: &QualifiedRoot, entry: &PublishedElection) -> usize {
        bucket_index(entry.handle.behavior(), entry.priority.balance)
    }

    fn compare_lowest_priority(
        &self,
        left_root: &QualifiedRoot,
        left: &PublishedElection,
        right_root: &QualifiedRoot,
        right: &PublishedElection,
    ) -> CmpOrdering {
        left.priority
            .time
            .cmp(&right.priority.time)
            .then_with(|| left.priority.balance.cmp(&right.priority.balance))
            .then_with(|| left_root.cmp(right_root))
    }

    fn bucket_handles(&self, bucket_id: usize) -> Vec<(QualifiedRoot, ElectionHandle)> {
        let mut entries: Vec<_> = self
            .published
            .load()
            .iter()
            .filter(|(root, entry)| self.bucket_of_entry(root, entry) == bucket_id)
            .map(|(root, entry)| (root.clone(), entry.clone()))
            .collect();
        entries.sort_by(|(left_root, left), (right_root, right)| {
            right
                .priority
                .time
                .cmp(&left.priority.time)
                .then_with(|| right.priority.balance.cmp(&left.priority.balance))
                .then_with(|| right_root.cmp(left_root))
        });
        entries
            .into_iter()
            .map(|(root, entry)| (root, entry.handle))
            .collect()
    }

    fn round_robin_handles(&self) -> Vec<(usize, QualifiedRoot, ElectionHandle)> {
        let mut buckets = vec![Vec::<(QualifiedRoot, PublishedElection)>::new(); bucket_count()];
        for (root, entry) in self.published.load().iter() {
            let bucket_id = self.bucket_of_entry(root, entry);
            buckets[bucket_id].push((root.clone(), entry.clone()));
        }

        for bucket in &mut buckets {
            bucket.sort_by(|(left_root, left), (right_root, right)| {
                right
                    .priority
                    .time
                    .cmp(&left.priority.time)
                    .then_with(|| right.priority.balance.cmp(&left.priority.balance))
                    .then_with(|| right_root.cmp(left_root))
            });
        }

        let mut positions = vec![0; bucket_count()];
        let mut result = Vec::new();
        loop {
            let mut progressed = false;
            for bucket_id in (0..bucket_count()).rev() {
                if let Some((root, entry)) = buckets[bucket_id].get(positions[bucket_id]).cloned() {
                    positions[bucket_id] += 1;
                    result.push((bucket_id, root, entry.handle));
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }
        result
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

    fn compare_exchange(&self, current: &Arc<T>, new: Arc<T>) -> bool {
        let current_ptr = Arc::as_ptr(current) as *mut T;
        let new_ptr = Arc::into_raw(new) as *mut T;
        match self
            .ptr
            .compare_exchange(current_ptr, new_ptr, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(old_ptr) => {
                unsafe {
                    drop(Arc::from_raw(old_ptr));
                }
                true
            }
            Err(_) => {
                unsafe {
                    drop(Arc::from_raw(new_ptr));
                }
                false
            }
        }
    }

    fn update(&self, f: impl Fn(&T) -> T) {
        loop {
            let current = self.load();
            let updated = Arc::new(f(&current));
            if self.compare_exchange(&current, updated) {
                return;
            }
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
        let mut cleanup_stats = StatsCollection::new();
        self.cleanup_stats
            .read()
            .unwrap()
            .collect_stats(&mut cleanup_stats);
        merge_stats(result, &cleanup_stats);
        self.vote_counter.collect_stats(result);
    }
}

impl ContainerInfoProvider for AecService {
    fn container_info(&self) -> ContainerInfo {
        let recently_confirmed_count = self.recently_confirmed.load().len();
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
    use rsnano_utils::sync::backpressure_channel::channel;
    use rsnano_utils::{container_info::ContainerInfoEntry, stats::StatsCollection};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Instant,
    };

    #[test]
    fn apply_vote_for_other_election_progresses_while_target_election_is_locked() {
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
            Vote::new(&rep_key, UnixMillisTimestamp::ZERO, 0, vec![block.hash()]).into(),
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
    fn insert_publishes_single_active_root() {
        let aec = AecService::new_null();
        let block = SavedBlock::new_test_instance();

        aec.insert(
            AecInsertRequest::new_priority(block.clone(), BlockPriority::new_test_instance()),
            Timestamp::new_test_instance(),
        )
        .unwrap();

        assert_eq!(aec.len(), 1);
        assert!(aec.is_active_root(&block.qualified_root()));
        assert!(aec.election_for_root(&block.qualified_root()).is_some());
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
    fn root_lookup_helpers_use_direct_published_roots() {
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

        let handle = aec
            .election_handle_for_root(&block_a.qualified_root())
            .unwrap();
        let election_guard = handle.lock();
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

        drop(election_guard);
        worker.join().unwrap();
    }

    #[test]
    fn activate_priority_replaces_lowest_election_across_independent_roots() {
        let aec = AecService::new_null();
        let block_a = SavedBlock::new_test_instance_with_key(1);
        let block_b = SavedBlock::new_test_instance_with_key(2);
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
    fn election_snapshots_collect_all_active_roots() {
        let aec = AecService::new_null();
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

        let seen: Vec<_> = aec
            .election_snapshots()
            .into_iter()
            .map(|election| election.qualified_root().clone())
            .collect();

        assert!(seen.contains(&block_a.qualified_root()));
        assert!(seen.contains(&block_b.qualified_root()));
    }

    #[test]
    fn container_info_reports_direct_root_count() {
        let aec = AecService::new_null();
        let block_a = SavedBlock::new_test_instance_with_key(1);
        let block_b = SavedBlock::new_test_instance_with_key(2);
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
    fn transition_active_for_other_election_progresses_while_target_election_is_locked() {
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

        drop(election_guard);
        worker.join().unwrap();
        assert_eq!(
            aec.election_for_block(&block_b.hash()).unwrap().state(),
            crate::consensus::election::ElectionState::Active
        );
    }

    #[test]
    fn confirm_dependent_elections_wait_only_for_the_target_election() {
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
