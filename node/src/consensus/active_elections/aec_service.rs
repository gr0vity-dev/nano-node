use std::{
    collections::HashMap,
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
};

use crate::{
    consensus::{
        ActiveElectionsConfig, ActiveElectionsContainer, AecActivateRequest, AecFact,
        AecInsertError, AecTickerRead, ApplyVoteArgs,
        ConfirmationActiveInfo, FilteredVote, ReceivedVote,
        election::{ConfirmedElection, Election, ElectionBehavior, ElectionState, VoteType},
        election_schedulers::priority::PriorityBucketState,
    },
    representatives::OnlineReps,
};

pub struct AecService {
    active: Arc<RwLock<ActiveElectionsContainer>>,
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
            self.count_by_behavior[i] = self.count_by_behavior[i].saturating_add_signed(*delta as isize);
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
            .node("recently_confirmed", self.recently_confirmed.container_info())
            .finish()
    }
}

impl AecService {
    const EVENT_QUEUE_SOFT_LIMIT: usize = 1024 * 5;

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
                active: Arc::new(RwLock::new(ActiveElectionsContainer::new(base_latency))),
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
        let mut active = self.active.write().unwrap();
        let result = active.erase(root);
        let erased = result.is_some();
        if let Some(result) = result {
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
        let total = self.active.read().unwrap().len();
        state.vacancy(total)
    }

    pub fn info(&self) -> crate::consensus::ActiveElectionsInfo {
        let state = self.state.lock().unwrap();
        let total = self.active.read().unwrap().len();
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
        let active = self.active.read().unwrap();
        for election in active.iter_round_robin() {
            if election.is_confirmed() {
                result.confirmed += 1;
            } else {
                result
                    .unconfirmed_roots
                    .push(election.qualified_root().clone());
            }
        }
        result
    }

    pub fn count_by_behavior(&self, behavior: ElectionBehavior) -> usize {
        self.state.lock().unwrap().count_by_behavior[behavior as usize]
    }

    pub fn is_active_root(&self, root: &QualifiedRoot) -> bool {
        self.active.read().unwrap().is_active_root(root)
    }

    pub fn is_active_hash(&self, hash: &BlockHash) -> bool {
        self.active.read().unwrap().is_active_hash(hash)
    }

    pub fn election_for_root(&self, root: &QualifiedRoot) -> Option<Election> {
        self.active.read().unwrap().election_for_root(root).cloned()
    }

    pub fn election_for_block(&self, hash: &BlockHash) -> Option<Election> {
        self.active
            .read()
            .unwrap()
            .election_for_block(hash)
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.active.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.active.read().unwrap().is_empty()
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
        let mut active = self.active.write().unwrap();
        let result = active.confirm_dependent_elections(confirmed, self.clock.now());
        state.apply(&result);
        self.publish_facts(result.facts);
    }

    pub(crate) fn try_add_fork(&self, fork: &Block, fork_tally: Amount) -> bool {
        let mut state = self.state.lock().unwrap();
        let mut active = self.active.write().unwrap();
        let (added, result) = active.try_add_fork(fork, fork_tally);
        state.apply(&result);
        self.publish_facts(result.facts);
        added
    }

    pub(crate) fn transition_time(&self) {
        let mut state = self.state.lock().unwrap();
        let mut active = self.active.write().unwrap();
        let result = active.transition_time(self.clock.now());
        state.apply(&result);
        self.publish_facts(result.facts);
    }

    pub fn transition_active(&self, block_hash: &BlockHash) -> bool {
        self.active.write().unwrap().transition_active(block_hash)
    }

    pub(crate) fn remove_votes(
        &self,
        root: &QualifiedRoot,
        voters: impl IntoIterator<Item = PublicKey>,
    ) {
        let voters = voters.into_iter().collect::<Vec<_>>();
        self.active
            .write()
            .unwrap()
            .remove_votes(root, voters.iter());
    }

    pub fn activate(&self, request: AecActivateRequest) -> Result<(), AecInsertError> {
        let mut state = self.state.lock().unwrap();
        let mut active = self.active.write().unwrap();
        match &request {
            AecActivateRequest::Manual { block, .. }
            | AecActivateRequest::Hinted { block, .. }
            | AecActivateRequest::Optimistic { block, .. }
            | AecActivateRequest::Priority { block, .. } => state.ensure_can_insert(block)?,
        }
        let active_len = active.len();
        let result = active.activate(
            request,
            self.clock.now(),
            state.cooldown.is_cooling_down(),
            state.vacancy(active_len),
        )?;
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
        let active = self.active.read().unwrap();
        active.priority_bucket_state(
            bucket,
            candidate_root,
            state.cooldown.is_cooling_down(),
            state.vacancy(active.len()),
        )
    }

    pub(in crate::consensus) fn next_vote_to_broadcast_for_voter(
        &self,
        bucket: usize,
        vote_broadcast_interval: Duration,
        now: Timestamp,
    ) -> Option<(Root, BlockHash, VoteType)> {
        self.active
            .write()
            .unwrap()
            .next_vote_to_broadcast(bucket, vote_broadcast_interval, now)
    }

    pub fn clear_recently_confirmed(&self) {
        self.state.lock().unwrap().recently_confirmed.clear();
    }

    pub fn force_confirm(&self, block_hash: &BlockHash) {
        let mut state = self.state.lock().unwrap();
        let mut active = self.active.write().unwrap();
        let result = active.force_confirm(block_hash, self.clock.now());
        state.apply(&result);
        self.publish_facts(result.facts);
    }

    pub fn cancel(&self, root: &QualifiedRoot) {
        self.active.write().unwrap().cancel(root);
    }

    pub fn cancel_all(&self) {
        self.active.write().unwrap().cancel_all();
    }

    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap();
        let mut active = self.active.write().unwrap();
        for election in active.iter_round_robin() {
            state.count_by_behavior[election.behavior() as usize] =
                state.count_by_behavior[election.behavior() as usize].saturating_sub(1);
            state.stats.stopped(election);
        }
        state.stopped = true;
        active.stop();
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
            let active = self.active.read().unwrap();
            vote.filtered_blocks()
                .any(|hash| active.is_active_hash(hash))
        };

        let now = self.clock.now();
        let quorum_specs = {
            let mut online = self.online_reps.lock().unwrap();
            if is_active {
                online.vote_observed(vote.voter, now);
            }
            online.quorum_specs()
        };

        let per_block = {
            let mut state = self.state.lock().unwrap();
            let mut active = self.active.write().unwrap();
            let rep_weights = self.rep_weights.read();
            let was_recently_confirmed =
                |hash: &BlockHash| state.recently_confirmed.hash_exists(hash);
            let result = active.apply_vote(
                ApplyVoteArgs {
                    vote,
                    rep_weights: &rep_weights,
                    quorum_specs: &quorum_specs,
                    now,
                },
                &was_recently_confirmed,
            );
            let mutation = AecMutationResult {
                facts: result.facts,
                delta: result.delta,
            };
            state.apply(&mutation);
            self.publish_facts(mutation.facts);
            result.per_block
        };
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
        let mut active = self.active.write().unwrap();
        match &request {
            AecActivateRequest::Manual { block, .. }
            | AecActivateRequest::Hinted { block, .. }
            | AecActivateRequest::Optimistic { block, .. }
            | AecActivateRequest::Priority { block, .. } => state.ensure_can_insert(block)?,
        }
        let active_len = active.len();
        let result = active.activate(
            request,
            now,
            state.cooldown.is_cooling_down(),
            state.vacancy(active_len),
        )?;
        state.apply(&result);
        self.publish_facts(result.facts);
        Ok(())
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
        let active = self.active.read().unwrap();
        ContainerInfo::builder()
            .node("global", state.container_info())
            .node("active", active.container_info())
            .finish()
    }
}

impl AecTickerRead for AecService {
    fn for_each_confirmation_solicitation_election(&self, action: &mut dyn FnMut(&Election)) {
        let active = self.active.read().unwrap();
        for election in active.iter_round_robin() {
            if election.state() == ElectionState::Active {
                action(election);
            }
        }
    }

    fn for_each_stale_election(
        &self,
        now: Timestamp,
        stale_threshold: Duration,
        action: &mut dyn FnMut(&Election),
    ) {
        let active = self.active.read().unwrap();
        for election in active.iter_round_robin() {
            if election.start().elapsed(now) >= stale_threshold {
                action(election);
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
    fn activate_priority_replaces_active_root_via_aec_owned_path() {
        let service = AecService::new_null();
        let old_block = SavedBlock::new_test_instance_with_key(1);
        let new_block = SavedBlock::new_test_instance_with_key(2);
        let old_root = old_block.qualified_root();
        let new_root = new_block.qualified_root();
        let priority = BlockPriority::new_test_instance();
        let bucket_index = prio_bucket_index(priority.balance);

        service
            .activate_for_test(
                AecActivateRequest::priority(
                    old_block,
                    priority,
                    bucket_index,
                    1,
                ),
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
}
