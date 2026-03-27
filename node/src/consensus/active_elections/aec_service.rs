use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use rsnano_ledger::RepWeightCache;
use rsnano_nullable_clock::{SteadyClock, Timestamp};
use rsnano_types::{
    Amount, Block, BlockHash, PublicKey, QualifiedRoot, Root, SavedBlock, VoteError,
};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{StatsCollection, StatsSource},
};
use strum::{EnumCount, IntoEnumIterator};

use super::{
    AecDelivery, AecFacts,
    active_elections_container::AecMutationResult,
    cooldown_controller::{AecCooldownReason, CooldownController, CooldownResult},
    recently_confirmed_cache::RecentlyConfirmedCache,
    stats::AecStats,
    vote_router::VoteRouter,
};

use crate::{
    consensus::{
        ActiveElectionsConfig, ActiveElectionsContainer, AecActivateRequest, AecFact,
        AecInsertError, AecTickerRead, ApplyVoteArgs, ConfirmationActiveInfo, FilteredVote,
        ReceivedVote,
        election::{ConfirmedElection, Election, ElectionBehavior, ElectionState, VoteType},
        election_schedulers::priority::PriorityBucketState,
    },
    representatives::OnlineReps,
};

pub struct AecService {
    router: RwLock<VoteRouter>,
    shards: Vec<RwLock<ActiveElectionsContainer>>,
    state: Mutex<AecGlobalState>,
    delivery: Arc<AecDelivery>,
    online_reps: Arc<Mutex<OnlineReps>>,
    clock: Arc<SteadyClock>,
    rep_weights: Arc<RepWeightCache>,
    is_dev_network: bool,
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

    fn ensure_can_insert(&self, block: &SavedBlock) -> Result<(), AecInsertError> {
        if self.stopped {
            return Err(AecInsertError::Stopped);
        }
        if self.recently_confirmed.root_exists(&block.qualified_root()) {
            return Err(AecInsertError::RecentlyConfirmed);
        }
        Ok(())
    }

    fn apply(&mut self, result: &AecMutationResult) {
        for (i, delta) in result.delta.behavior_counts.iter().enumerate() {
            self.count_by_behavior[i] =
                self.count_by_behavior[i].saturating_add_signed(*delta as isize);
        }
        for behavior in ElectionBehavior::iter() {
            for _ in 0..result.delta.started_behaviors[behavior as usize] {
                self.stats.started(behavior);
            }
        }
        for election in &result.delta.stopped_elections {
            self.stats.stopped(election);
        }
        for (root, hash) in &result.delta.recently_confirmed {
            self.recently_confirmed.put(root.clone(), *hash);
        }
        self.stats.vote_counter.add_counts(result.delta.vote_counts);
        self.stats.ticked += result.delta.ticked;
        self.stats.conflicts += result.delta.conflicts;
        for (i, count) in result.delta.block_confirmations.iter().enumerate() {
            self.stats.block_confirmations[i] += count;
        }
    }

    fn max_len(&self) -> usize {
        self.max_elections
    }

    fn vacancy(&self, current_size: usize) -> i64 {
        if self.cooldown.is_cooling_down() {
            0
        } else {
            self.max_elections as i64 - current_size as i64
        }
    }

    fn info(&self, total: usize) -> crate::consensus::ActiveElectionsInfo {
        crate::consensus::ActiveElectionsInfo {
            max_elections: self.max_elections,
            total,
            priority: self.count_by_behavior[ElectionBehavior::Priority as usize],
            hinted: self.count_by_behavior[ElectionBehavior::Hinted as usize],
            optimistic: self.count_by_behavior[ElectionBehavior::Optimistic as usize],
        }
    }
}

impl StatsSource for AecGlobalState {
    fn collect_stats(&self, result: &mut StatsCollection) {
        self.cooldown.collect_stats(result);
        self.stats.collect_stats(result);
    }
}

impl ContainerInfoProvider for AecGlobalState {
    fn container_info(&self) -> ContainerInfo {
        ContainerInfo::builder()
            .leaf(
                "normal",
                self.count_by_behavior[ElectionBehavior::Priority as usize],
                0,
            )
            .leaf(
                "hinted".to_string(),
                self.count_by_behavior[ElectionBehavior::Hinted as usize],
                0,
            )
            .leaf(
                "optimistic".to_string(),
                self.count_by_behavior[ElectionBehavior::Optimistic as usize],
                0,
            )
            .node(
                "recently_confirmed",
                self.recently_confirmed.container_info(),
            )
            .finish()
    }
}

impl AecService {
    const EVENT_QUEUE_SOFT_LIMIT: usize = 1024 * 5;
    const SHARD_COUNT: usize = 8;

    pub fn new(
        config: ActiveElectionsConfig,
        base_latency: Duration,
        online_reps: Arc<Mutex<OnlineReps>>,
        clock: Arc<SteadyClock>,
        rep_weights: Arc<RepWeightCache>,
        is_dev_network: bool,
    ) -> Self {
        Self::new_with_delivery(
            config,
            base_latency,
            online_reps,
            clock,
            rep_weights,
            is_dev_network,
        )
        .0
    }

    pub(crate) fn new_with_delivery(
        config: ActiveElectionsConfig,
        base_latency: Duration,
        online_reps: Arc<Mutex<OnlineReps>>,
        clock: Arc<SteadyClock>,
        rep_weights: Arc<RepWeightCache>,
        is_dev_network: bool,
    ) -> (Self, Arc<AecDelivery>) {
        let delivery = Arc::new(AecDelivery::new(Self::EVENT_QUEUE_SOFT_LIMIT));
        (
            Self {
                router: RwLock::new(VoteRouter::default()),
                shards: (0..Self::SHARD_COUNT)
                    .map(|_| RwLock::new(ActiveElectionsContainer::new(base_latency)))
                    .collect(),
                state: Mutex::new(AecGlobalState::new(config)),
                delivery: delivery.clone(),
                online_reps,
                clock,
                rep_weights,
                is_dev_network,
            },
            delivery,
        )
    }

    pub fn new_null() -> Self {
        Self::new_null_with_delivery().0
    }

    pub(crate) fn new_null_with_delivery() -> (Self, Arc<AecDelivery>) {
        let rep_weights = Arc::new(RepWeightCache::default());
        let online_reps = Arc::new(Mutex::new(
            OnlineReps::builder()
                .rep_weights(rep_weights.clone())
                .finish(),
        ));
        Self::new_with_delivery(
            ActiveElectionsConfig::default(),
            Duration::from_secs(1),
            online_reps,
            Arc::new(SteadyClock::new_null()),
            rep_weights,
            false,
        )
    }

    pub(crate) fn event_queue_len(&self) -> usize {
        self.delivery.queue_len()
    }

    pub(crate) fn set_cooldown(&self, cool_down: bool, reason: AecCooldownReason) {
        let mut state = self.state.lock().unwrap();
        let result = state.cooldown.set_cooldown(cool_down, reason);
        let facts = if result == CooldownResult::Recovered {
            AecFact::Recovered.into()
        } else {
            AecFacts::new()
        };
        self.publish_facts(facts);
    }

    pub fn erase(&self, root: &QualifiedRoot) -> bool {
        let mut state = self.state.lock().unwrap();
        let mut shard = self.shard_for_root(root).write().unwrap();
        let result = shard.erase(root);
        let erased = result.is_some();
        if let Some(result) = result {
            self.apply_router_delta(&result);
            state.apply(&result);
            self.publish_facts(result.facts);
        }
        erased
    }

    pub fn max_len(&self) -> usize {
        self.state.lock().unwrap().max_len()
    }

    // Shared AEC query surface used by production callers and tests.
    // These methods answer AEC-shaped questions without exposing caller-specific
    // traversal or runtime helpers.
    pub fn vacancy(&self) -> i64 {
        let state = self.state.lock().unwrap();
        let total = self.len();
        state.vacancy(total)
    }

    pub fn info(&self) -> crate::consensus::ActiveElectionsInfo {
        let state = self.state.lock().unwrap();
        let total = self.len();
        state.info(total)
    }

    pub fn was_recently_confirmed(&self, block_hash: &BlockHash) -> bool {
        self.state
            .lock()
            .unwrap()
            .recently_confirmed
            .hash_exists(block_hash)
    }

    pub fn confirmation_active(&self, announcements: u64) -> ConfirmationActiveInfo {
        if announcements > 0 {
            return ConfirmationActiveInfo::default();
        }

        let mut result = ConfirmationActiveInfo::default();
        for shard in &self.shards {
            let shard = shard.read().unwrap();
            for election in shard.iter_round_robin() {
                if election.is_confirmed() {
                    result.confirmed += 1;
                } else {
                    result
                        .unconfirmed_roots
                        .push(election.qualified_root().clone());
                }
            }
        }
        result
    }

    pub fn count_by_behavior(&self, behavior: ElectionBehavior) -> usize {
        self.state.lock().unwrap().count_by_behavior[behavior as usize]
    }

    pub fn is_active_root(&self, root: &QualifiedRoot) -> bool {
        self.shard_for_root(root)
            .read()
            .unwrap()
            .is_active_root(root)
    }

    pub fn is_active_hash(&self, hash: &BlockHash) -> bool {
        self.router.read().unwrap().is_active(hash)
    }

    pub fn election_for_root(&self, root: &QualifiedRoot) -> Option<Election> {
        self.shard_for_root(root)
            .read()
            .unwrap()
            .election_for_root(root)
            .cloned()
    }

    pub fn election_for_block(&self, hash: &BlockHash) -> Option<Election> {
        let root = self.router.read().unwrap().qualified_root(hash).cloned()?;
        self.election_for_root(&root)
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

    pub(crate) fn remove_recently_confirmed(&self, block_hash: &BlockHash) {
        self.state
            .lock()
            .unwrap()
            .recently_confirmed
            .erase(block_hash);
    }

    // Shared AEC mutation surface. Caller-specific helpers below are temporary and
    // are removed unit by unit as the boundary is narrowed.
    pub(crate) fn confirm_dependent_elections(
        &self,
        confirmed: Vec<(SavedBlock, Option<ConfirmedElection>)>,
    ) {
        let mut state = self.state.lock().unwrap();
        let mut result = AecMutationResult::default();
        let mut by_shard = vec![Vec::new(); self.shards.len()];
        for (confirmed_block, source_election) in confirmed {
            let shard_index = self.shard_index(&confirmed_block.qualified_root());
            by_shard[shard_index].push((confirmed_block, source_election));
        }
        for (shard_index, shard_confirmed) in by_shard.into_iter().enumerate() {
            if shard_confirmed.is_empty() {
                continue;
            }
            let shard_result = self.shards[shard_index]
                .write()
                .unwrap()
                .confirm_dependent_elections(shard_confirmed, self.clock.now());
            result.merge(shard_result);
        }
        self.apply_router_delta(&result);
        state.apply(&result);
        self.publish_facts(result.facts);
    }

    pub(crate) fn try_add_fork(&self, fork: &Block, fork_tally: Amount) -> bool {
        let mut state = self.state.lock().unwrap();
        let root = fork.qualified_root();
        let mut shard = self.shard_for_root(&root).write().unwrap();
        let (added, result) = shard.try_add_fork(fork, fork_tally);
        self.apply_router_delta(&result);
        state.apply(&result);
        self.publish_facts(result.facts);
        added
    }

    pub(crate) fn transition_time(&self) {
        let mut state = self.state.lock().unwrap();
        let mut result = AecMutationResult::default();
        for shard in &self.shards {
            result.merge(shard.write().unwrap().transition_time(self.clock.now()));
        }
        self.apply_router_delta(&result);
        state.apply(&result);
        self.publish_facts(result.facts);
    }

    pub fn transition_active(&self, block_hash: &BlockHash) -> bool {
        let Some(root) = self
            .router
            .read()
            .unwrap()
            .qualified_root(block_hash)
            .cloned()
        else {
            return false;
        };
        self.shard_for_root(&root)
            .write()
            .unwrap()
            .transition_active(&root)
    }

    pub(crate) fn remove_votes(
        &self,
        root: &QualifiedRoot,
        voters: impl IntoIterator<Item = PublicKey>,
    ) {
        let voters = voters.into_iter().collect::<Vec<_>>();
        self.shard_for_root(root)
            .write()
            .unwrap()
            .remove_votes(root, voters.iter());
    }

    pub fn activate(&self, request: AecActivateRequest) -> Result<(), AecInsertError> {
        let mut state = self.state.lock().unwrap();
        let active_len = self.len();
        let result = self.activate_with_state(&mut state, request, self.clock.now(), active_len)?;
        self.apply_router_delta(&result);
        state.apply(&result);
        self.publish_facts(result.facts);
        Ok(())
    }

    pub(crate) fn priority_bucket_state(
        &self,
        bucket: usize,
        candidate_root: &QualifiedRoot,
    ) -> PriorityBucketState {
        let state = self.state.lock().unwrap();
        self.global_priority_bucket_state(bucket, candidate_root, &state, self.len())
    }

    pub(in crate::consensus) fn next_vote_to_broadcast_for_voter(
        &self,
        bucket: usize,
        vote_broadcast_interval: Duration,
        now: Timestamp,
    ) -> Option<(Root, BlockHash, VoteType)> {
        for shard in &self.shards {
            let next =
                shard
                    .write()
                    .unwrap()
                    .next_vote_to_broadcast(bucket, vote_broadcast_interval, now);
            if next.is_some() {
                return next;
            }
        }
        None
    }

    pub fn clear_recently_confirmed(&self) {
        self.state.lock().unwrap().recently_confirmed.clear();
    }

    pub fn force_confirm(&self, block_hash: &BlockHash) {
        let mut state = self.state.lock().unwrap();
        let root = self
            .router
            .read()
            .unwrap()
            .qualified_root(block_hash)
            .cloned()
            .expect("Force confirm failed, because no active election was found");
        let mut shard = self.shard_for_root(&root).write().unwrap();
        let result = shard.force_confirm(&root, self.clock.now());
        state.apply(&result);
        self.publish_facts(result.facts);
    }

    pub fn cancel(&self, root: &QualifiedRoot) {
        self.shard_for_root(root).write().unwrap().cancel(root);
    }

    pub fn cancel_all(&self) {
        for shard in &self.shards {
            shard.write().unwrap().cancel_all();
        }
    }

    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap();
        for shard in &self.shards {
            let mut shard = shard.write().unwrap();
            for election in shard.iter_round_robin() {
                state.count_by_behavior[election.behavior() as usize] =
                    state.count_by_behavior[election.behavior() as usize].saturating_sub(1);
                state.stats.stopped(election);
            }
            shard.stop();
        }
        state.stopped = true;
        self.router.write().unwrap().clear();
    }

    pub(crate) fn apply_vote(
        &self,
        vote: &FilteredVote,
    ) -> HashMap<BlockHash, Result<(), VoteError>> {
        debug_assert!(vote.validate().is_ok());

        let minimum_pr_weight = self.online_reps.lock().unwrap().minimum_principal_weight();
        let voter_weight = self.rep_weights.weight(&vote.voter);

        if !self.is_dev_network && voter_weight <= minimum_pr_weight {
            return vote
                .filtered_blocks()
                .map(|hash| (*hash, Err(VoteError::Indeterminate)))
                .collect();
        }

        let is_active = {
            let active = self.router.read().unwrap();
            vote.filtered_blocks().any(|hash| active.is_active(hash))
        };

        let now = self.clock.now();
        let quorum_specs = {
            let mut online = self.online_reps.lock().unwrap();
            if is_active {
                online.vote_observed(vote.voter, now);
            }
            online.quorum_specs()
        };

        let vote_plan = self.plan_vote(vote);
        let mut per_block = vote_plan.unrouted_results;
        let recently_confirmed =
            self.recently_confirmed_snapshot(vote_plan.routed_hashes.iter().copied());
        let rep_weights = self.rep_weights.read();

        for shard_targets in vote_plan.routed_by_shard.into_values() {
            let mut shard = self.shards[shard_targets.shard_index].write().unwrap();
            let router = self.router.read().unwrap();
            let was_recently_confirmed =
                |hash: &BlockHash| recently_confirmed.get(hash).copied().unwrap_or(false);
            let mut mutation = AecMutationResult::default();

            for block_hash in shard_targets.block_hashes {
                let filtered_vote = FilteredVote::new(vote.vote.clone(), block_hash);
                let result = shard.apply_vote(
                    ApplyVoteArgs {
                        vote: &filtered_vote,
                        rep_weights: &rep_weights,
                        quorum_specs: &quorum_specs,
                        now,
                    },
                    &router,
                    &was_recently_confirmed,
                );
                per_block.extend(result.per_block);
                mutation.merge(AecMutationResult {
                    facts: result.facts,
                    delta: result.delta,
                });
            }

            drop(router);
            drop(shard);

            let mut state = self.state.lock().unwrap();
            self.apply_router_delta(&mutation);
            state.apply(&mutation);
            self.publish_facts(mutation.facts);
        }
        self.notify_vote_processed(vote.vote.clone(), voter_weight, &per_block);
        per_block
    }

    fn publish_facts(&self, facts: AecFacts) {
        for fact in facts {
            self.send_fact(fact);
        }
    }

    fn notify_vote_processed(
        &self,
        vote: ReceivedVote,
        voter_weight: Amount,
        results: &HashMap<BlockHash, Result<(), VoteError>>,
    ) {
        self.send_fact(AecFact::VoteProcessed(vote, voter_weight, results.clone()));
    }

    fn send_fact(&self, fact: AecFact) {
        self.delivery.publish(fact);
    }

    #[cfg(test)]
    pub(crate) fn activate_for_test(
        &self,
        request: AecActivateRequest,
        now: Timestamp,
    ) -> Result<(), AecInsertError> {
        let mut state = self.state.lock().unwrap();
        let active_len = self.len();
        let result = self.activate_with_state(&mut state, request, now, active_len)?;
        self.apply_router_delta(&result);
        state.apply(&result);
        self.publish_facts(result.facts);
        Ok(())
    }

    fn shard_for_root(&self, root: &QualifiedRoot) -> &RwLock<ActiveElectionsContainer> {
        &self.shards[self.shard_index(root)]
    }

    fn shard_index(&self, root: &QualifiedRoot) -> usize {
        usize::from(root.to_bytes()[0]) % self.shards.len()
    }

    fn plan_vote(&self, vote: &FilteredVote) -> VotePlan {
        let mut unrouted = Vec::new();
        let mut routed_by_shard = HashMap::<usize, ShardVoteTargets>::new();
        let mut unrouted_results = HashMap::new();
        let mut seen = HashSet::new();

        {
            let router = self.router.read().unwrap();
            for block_hash in vote.filtered_blocks() {
                if !seen.insert(*block_hash) {
                    continue;
                }

                match router.qualified_root(block_hash).cloned() {
                    Some(root) => {
                        let shard_index = self.shard_index(&root);
                        routed_by_shard
                            .entry(shard_index)
                            .or_insert_with(|| ShardVoteTargets::new(shard_index))
                            .block_hashes
                            .push(*block_hash);
                    }
                    None => unrouted.push(*block_hash),
                }
            }
        }

        if !unrouted.is_empty() {
            let state = self.state.lock().unwrap();
            for block_hash in unrouted {
                let error = if state.recently_confirmed.hash_exists(&block_hash) {
                    VoteError::Late
                } else {
                    VoteError::Indeterminate
                };
                unrouted_results.insert(block_hash, Err(error));
            }
        }

        let routed_hashes = routed_by_shard
            .values()
            .flat_map(|targets| targets.block_hashes.iter().copied())
            .collect();

        VotePlan {
            routed_by_shard,
            routed_hashes,
            unrouted_results,
        }
    }

    fn recently_confirmed_snapshot(
        &self,
        hashes: impl IntoIterator<Item = BlockHash>,
    ) -> HashMap<BlockHash, bool> {
        let state = self.state.lock().unwrap();
        hashes
            .into_iter()
            .map(|hash| (hash, state.recently_confirmed.hash_exists(&hash)))
            .collect()
    }

    fn apply_router_delta(&self, result: &AecMutationResult) {
        let mut router = self.router.write().unwrap();
        for hash in &result.delta.route_removals {
            router.disconnect(hash);
        }
        for (hash, root) in &result.delta.route_additions {
            router.connect(*hash, root.clone());
        }
    }

    fn activate_with_state(
        &self,
        state: &mut AecGlobalState,
        request: AecActivateRequest,
        now: Timestamp,
        active_len: usize,
    ) -> Result<AecMutationResult, AecInsertError> {
        match &request {
            AecActivateRequest::Manual { block, .. }
            | AecActivateRequest::Hinted { block, .. }
            | AecActivateRequest::Optimistic { block, .. }
            | AecActivateRequest::Priority { block, .. } => state.ensure_can_insert(block)?,
        }

        match request {
            AecActivateRequest::Priority {
                block,
                priority,
                bucket_index,
                reserved_elections,
            } => self.activate_priority(
                block,
                priority,
                bucket_index,
                reserved_elections,
                now,
                state,
                active_len,
            ),
            request => {
                let root = request.qualified_root();
                self.shard_for_root(&root).write().unwrap().activate(
                    request,
                    now,
                    state.cooldown.is_cooling_down(),
                    state.vacancy(active_len),
                )
            }
        }
    }

    fn activate_priority(
        &self,
        block: SavedBlock,
        priority: rsnano_types::BlockPriority,
        bucket_index: usize,
        reserved_elections: usize,
        now: Timestamp,
        state: &AecGlobalState,
        active_len: usize,
    ) -> Result<AecMutationResult, AecInsertError> {
        let candidate_root = block.qualified_root();
        let bucket_state =
            self.global_priority_bucket_state(bucket_index, &candidate_root, state, active_len);
        let request =
            AecActivateRequest::priority(block, priority, bucket_index, reserved_elections);

        if bucket_state.contains_candidate {
            return self
                .shard_for_root(&candidate_root)
                .write()
                .unwrap()
                .activate(
                    request,
                    now,
                    state.cooldown.is_cooling_down(),
                    state.vacancy(active_len),
                );
        }

        if bucket_state.active_len >= reserved_elections {
            let Some((lowest_root, _)) = bucket_state.lowest else {
                debug_assert!(false, "priority replacement requires a lowest election");
                return Err(AecInsertError::Duplicate);
            };
            let request = request.into_insert_request();
            let mut result = self
                .shard_for_root(&lowest_root)
                .write()
                .unwrap()
                .erase(&lowest_root)
                .ok_or(AecInsertError::Duplicate)?;
            let new_root = request.block.qualified_root();
            let insert = self
                .shard_for_root(&new_root)
                .write()
                .unwrap()
                .insert(request, now)?;
            result.merge(insert);
            Ok(result)
        } else {
            self.shard_for_root(&candidate_root)
                .write()
                .unwrap()
                .activate(
                    request,
                    now,
                    state.cooldown.is_cooling_down(),
                    state.vacancy(active_len),
                )
        }
    }

    fn global_priority_bucket_state(
        &self,
        bucket: usize,
        candidate_root: &QualifiedRoot,
        state: &AecGlobalState,
        active_len: usize,
    ) -> PriorityBucketState {
        let mut result = PriorityBucketState {
            active_len: 0,
            contains_candidate: self.is_active_root(candidate_root),
            lowest: None,
            is_cooling_down: state.cooldown.is_cooling_down(),
            vacancy: state.vacancy(active_len),
        };

        for shard in &self.shards {
            let shard = shard.read().unwrap();
            let shard_state = shard.priority_bucket_state(
                bucket,
                candidate_root,
                result.is_cooling_down,
                result.vacancy,
            );
            result.active_len += shard_state.active_len;
            result.lowest = match (result.lowest.take(), shard_state.lowest) {
                (None, other) => other,
                (current @ Some(_), None) => current,
                (Some((current_root, current_time)), Some((next_root, next_time))) => {
                    if next_time < current_time {
                        Some((next_root, next_time))
                    } else {
                        Some((current_root, current_time))
                    }
                }
            };
        }

        result
    }
}

struct VotePlan {
    routed_by_shard: HashMap<usize, ShardVoteTargets>,
    routed_hashes: Vec<BlockHash>,
    unrouted_results: HashMap<BlockHash, Result<(), VoteError>>,
}

struct ShardVoteTargets {
    shard_index: usize,
    block_hashes: Vec<BlockHash>,
}

impl ShardVoteTargets {
    fn new(shard_index: usize) -> Self {
        Self {
            shard_index,
            block_hashes: Vec::new(),
        }
    }
}

impl StatsSource for AecService {
    fn collect_stats(&self, result: &mut StatsCollection) {
        self.state.lock().unwrap().collect_stats(result);
    }
}

impl ContainerInfoProvider for AecService {
    fn container_info(&self) -> ContainerInfo {
        let state = self.state.lock().unwrap();
        let active: usize = self
            .shards
            .iter()
            .map(|shard| shard.read().unwrap().len())
            .sum();
        ContainerInfo::builder()
            .node("global", state.container_info())
            .leaf("active", active, 0)
            .node("vote_router", self.router.read().unwrap().container_info())
            .finish()
    }
}

impl AecTickerRead for AecService {
    fn for_each_confirmation_solicitation_election(&self, action: &mut dyn FnMut(&Election)) {
        for shard in &self.shards {
            let shard = shard.read().unwrap();
            for election in shard.iter_round_robin() {
                if election.state() == ElectionState::Active {
                    action(election);
                }
            }
        }
    }

    fn for_each_stale_election(
        &self,
        now: Timestamp,
        stale_threshold: Duration,
        action: &mut dyn FnMut(&Election),
    ) {
        for shard in &self.shards {
            let shard = shard.read().unwrap();
            for election in shard.iter_round_robin() {
                if election.start().elapsed(now) >= stale_threshold {
                    action(election);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::consensus::{
        AecActivateRequest, AecFact, election_schedulers::priority::prio_bucket_index,
    };
    use crate::utils::BackpressureEventProcessor;
    use rsnano_types::{
        BlockPriority, PrivateKey, SavedBlock, UnixMillisTimestamp, Vote, VoteSource,
    };
    use std::sync::mpsc::TryRecvError;

    use super::*;

    #[test]
    fn construction_does_not_start_processing_implicitly() {
        let (service, delivery) = AecService::new_null_with_delivery();

        delivery.publish(AecFact::Recovered);

        assert_eq!(service.event_queue_len(), 1);
    }

    #[test]
    fn processing_starts_only_when_explicitly_requested() {
        let (_service, delivery) = AecService::new_null_with_delivery();
        let processor = StubProcessor::default();

        delivery.publish(AecFact::Recovered);
        assert!(processor.log().is_empty());

        delivery.start_event_processor("aec-service-test", processor.clone());

        let start = std::time::Instant::now();
        while processor.log().is_empty() && start.elapsed() < Duration::from_secs(5) {
            std::thread::yield_now();
        }

        assert_eq!(processor.log(), vec!["processed"]);
    }

    #[test]
    fn stop_closes_event_queue_and_rejects_further_publication() {
        let (service, delivery) = AecService::new_null_with_delivery();

        delivery.stop();
        service.stop();
        delivery.publish(AecFact::Recovered);

        assert_eq!(service.event_queue_len(), 0);
        assert!(matches!(
            delivery.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn apply_vote_publishes_vote_processed_event() {
        let service = AecService::new_null();
        let rep_key = PrivateKey::from(1);
        let block = SavedBlock::new_test_instance();
        let block_hash = block.hash();

        service.rep_weights.put(rep_key.public_key(), Amount::MAX);
        service
            .activate_for_test(
                AecActivateRequest::priority(
                    block,
                    BlockPriority::new_test_instance(),
                    prio_bucket_index(BlockPriority::new_test_instance().balance),
                    1,
                ),
                service.clock.now(),
            )
            .unwrap();

        let vote = ReceivedVote::new(
            Vote::new(&rep_key, UnixMillisTimestamp::new(123), 0, vec![block_hash]).into(),
            VoteSource::Live,
            None,
        );

        let results = service.apply_vote(&vote.clone().into());
        assert_eq!(results.get(&block_hash), Some(&Ok(())));

        let mut observed_vote_processed = false;
        let start = std::time::Instant::now();

        while start.elapsed() < Duration::from_secs(5) {
            match service.delivery.try_recv() {
                Ok(AecFact::VoteProcessed(processed_vote, voter_weight, per_block_results)) => {
                    assert_eq!(processed_vote.vote.hashes, vote.vote.hashes);
                    assert_eq!(voter_weight, Amount::MAX);
                    assert_eq!(per_block_results.get(&block_hash), Some(&Ok(())));
                    observed_vote_processed = true;
                    break;
                }
                Ok(
                    AecFact::ElectionStarted(_, _)
                    | AecFact::ElectionConfirmed(_)
                    | AecFact::ElectionEnded(_),
                ) => {}
                Ok(other) => panic!("unexpected event: {:?}", std::mem::discriminant(&other)),
                Err(TryRecvError::Empty) => std::thread::yield_now(),
                Err(TryRecvError::Disconnected) => break,
            }
        }

        assert!(observed_vote_processed);
    }

    #[test]
    fn apply_vote_reports_every_hash_across_shards() {
        let service = AecService::new_null();
        let rep_key = PrivateKey::from(1);
        service.rep_weights.put(rep_key.public_key(), Amount::MAX);

        let block_a = SavedBlock::new_test_instance_with_key(1);
        let block_b = block_in_different_shard(&service, &block_a);
        let priority = BlockPriority::new_test_instance();
        let bucket_index = prio_bucket_index(priority.balance);

        service
            .activate_for_test(
                AecActivateRequest::priority(block_a.clone(), priority, bucket_index, 2),
                service.clock.now(),
            )
            .unwrap();
        service
            .activate_for_test(
                AecActivateRequest::priority(block_b.clone(), priority, bucket_index, 2),
                service.clock.now(),
            )
            .unwrap();

        let vote = ReceivedVote::new(
            Vote::new(
                &rep_key,
                UnixMillisTimestamp::new(123),
                0,
                vec![block_a.hash(), block_b.hash()],
            )
            .into(),
            VoteSource::Live,
            None,
        );

        let results = service.apply_vote(&vote.into());

        assert_eq!(results.get(&block_a.hash()), Some(&Ok(())));
        assert_eq!(results.get(&block_b.hash()), Some(&Ok(())));
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn confirm_dependent_elections_emits_one_block_confirmed_fact_after_sharding() {
        let (service, delivery) = AecService::new_null_with_delivery();
        let block = SavedBlock::new_test_instance();

        service.confirm_dependent_elections(vec![(block.clone(), None)]);

        let mut confirmed = Vec::new();
        while let Ok(fact) = delivery.try_recv() {
            if let AecFact::BlockConfirmed(confirmed_block, election) = fact {
                confirmed.push((confirmed_block, election.confirmation_type));
            }
        }

        assert_eq!(
            confirmed,
            vec![(
                block,
                crate::consensus::election::ConfirmationType::InactiveConfirmationHeight,
            )]
        );
    }

    #[test]
    fn activate_priority_replaces_active_root_via_aec_owned_path() {
        let service = AecService::new_null();
        let old_block = SavedBlock::new_test_instance_with_key(1);
        let new_block = block_in_different_shard(&service, &old_block);
        let old_root = old_block.qualified_root();
        let new_root = new_block.qualified_root();
        let new_hash = new_block.hash();
        let priority = BlockPriority::new_test_instance();
        let bucket_index = prio_bucket_index(priority.balance);

        service
            .activate_for_test(
                AecActivateRequest::priority(old_block, priority, bucket_index, 1),
                service.clock.now(),
            )
            .unwrap();

        service
            .activate(AecActivateRequest::priority(
                new_block,
                priority,
                bucket_index,
                1,
            ))
            .unwrap();

        assert!(!service.is_active_root(&old_root));
        assert!(service.is_active_root(&new_root));
        assert!(service.is_active_hash(&new_hash));
        assert_eq!(
            service
                .election_for_block(&new_hash)
                .map(|election| election.qualified_root().clone()),
            Some(new_root)
        );
    }

    #[test]
    fn force_confirm_updates_service_recently_confirmed_before_reactivation() {
        let service = AecService::new_null();
        let block = SavedBlock::new_test_instance();
        let root = block.qualified_root();
        let hash = block.hash();
        let priority = BlockPriority::new_test_instance();
        let bucket_index = prio_bucket_index(priority.balance);

        service
            .activate_for_test(
                AecActivateRequest::priority(block.clone(), priority, bucket_index, 1),
                service.clock.now(),
            )
            .unwrap();

        service.force_confirm(&hash);

        assert!(service.was_recently_confirmed(&hash));

        assert!(service.erase(&root));

        let result = service.activate(AecActivateRequest::priority(
            block,
            priority,
            bucket_index,
            1,
        ));

        assert_eq!(result, Err(AecInsertError::RecentlyConfirmed));
    }

    #[test]
    fn confirmation_active_returns_unconfirmed_roots_without_cloning_elections() {
        let service = AecService::new_null();
        let block = SavedBlock::new_test_instance();
        let root = block.qualified_root();

        service
            .activate_for_test(
                AecActivateRequest::priority(
                    block,
                    BlockPriority::new_test_instance(),
                    prio_bucket_index(BlockPriority::new_test_instance().balance),
                    1,
                ),
                service.clock.now(),
            )
            .unwrap();

        let result = service.confirmation_active(0);

        assert_eq!(result.unconfirmed_roots, vec![root]);
        assert_eq!(result.confirmed, 0);
    }

    #[test]
    fn confirmation_active_honors_announcement_threshold() {
        let service = AecService::new_null();
        let block = SavedBlock::new_test_instance();

        service
            .activate_for_test(
                AecActivateRequest::priority(
                    block,
                    BlockPriority::new_test_instance(),
                    prio_bucket_index(BlockPriority::new_test_instance().balance),
                    1,
                ),
                service.clock.now(),
            )
            .unwrap();

        let result = service.confirmation_active(1);

        assert!(result.unconfirmed_roots.is_empty());
        assert_eq!(result.confirmed, 0);
    }

    #[derive(Clone, Default)]
    struct StubProcessor {
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl StubProcessor {
        fn log(&self) -> Vec<&'static str> {
            self.log.lock().unwrap().clone()
        }
    }

    impl BackpressureEventProcessor<AecFact> for StubProcessor {
        fn cool_down(&mut self) {}

        fn recovered(&mut self) {
            self.log.lock().unwrap().push("recovered");
        }

        fn process(&mut self, _event: AecFact) {
            self.log.lock().unwrap().push("processed");
        }
    }

    fn block_in_different_shard(service: &AecService, other: &SavedBlock) -> SavedBlock {
        let other_shard = usize::from(other.qualified_root().to_bytes()[0]) % service.shards.len();
        for key in 2..=64 {
            let block = SavedBlock::new_test_instance_with_key(key);
            let shard = usize::from(block.qualified_root().to_bytes()[0]) % service.shards.len();
            if shard != other_shard {
                return block;
            }
        }
        panic!("expected test fixture blocks to span multiple shards");
    }
}
