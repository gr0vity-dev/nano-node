use std::{
    collections::HashMap,
    ops::Deref,
    time::{Duration, SystemTime},
};

use rsnano_core::{
    utils::{ContainerInfo, UnixMillisTimestamp},
    Amount, Block, BlockHash, PublicKey, QualifiedRoot, SavedBlock, VoteCode, VoteSource,
};
use rsnano_nullable_clock::Timestamp;

use crate::consensus::{
    AddForkResult, ConfirmedElection, Election, ElectionBehavior, ElectionResult, VoteSummary,
};

use super::{
    cooldown_controller::{AecCooldownReason, CooldownController},
    recently_confirmed_cache::RecentlyConfirmedCache,
    ActiveElectionsConfig, AecEvent, Entry, ErasedCallback, RootContainer, VoteRouter,
};

pub struct ActiveElectionsContainer {
    roots: RootContainer,
    stopped: bool,
    manual_count: usize,
    priority_count: usize,
    hinted_count: usize,
    optimistic_count: usize,
    base_latency: Duration,
    pub(super) vote_router: VoteRouter,
    pub(super) recently_confirmed: RecentlyConfirmedCache,
    cooldown: CooldownController,
    max_elections: usize,
}

impl ActiveElectionsContainer {
    pub fn new(config: ActiveElectionsConfig, base_latency: Duration) -> Self {
        Self {
            roots: RootContainer::default(),
            vote_router: VoteRouter::new(),
            stopped: false,
            manual_count: 0,
            priority_count: 0,
            hinted_count: 0,
            optimistic_count: 0,
            base_latency,
            recently_confirmed: RecentlyConfirmedCache::new(config.confirmation_cache),
            cooldown: CooldownController::new(),
            max_elections: config.max_elections,
        }
    }

    pub fn count_by_behavior(&self, behavior: ElectionBehavior) -> usize {
        match behavior {
            ElectionBehavior::Manual => self.manual_count,
            ElectionBehavior::Priority => self.priority_count,
            ElectionBehavior::Hinted => self.hinted_count,
            ElectionBehavior::Optimistic => self.optimistic_count,
        }
    }

    pub fn count_by_behavior_mut(&mut self, behavior: ElectionBehavior) -> &mut usize {
        match behavior {
            ElectionBehavior::Manual => &mut self.manual_count,
            ElectionBehavior::Priority => &mut self.priority_count,
            ElectionBehavior::Hinted => &mut self.hinted_count,
            ElectionBehavior::Optimistic => &mut self.optimistic_count,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Election> {
        self.roots.iter_sequenced().map(|i| &i.election)
    }

    pub(super) fn insert(
        &mut self,
        block: SavedBlock,
        election_behavior: ElectionBehavior,
        erased_callback: Option<ErasedCallback>,
        now: Timestamp,
    ) -> bool {
        let hash = block.hash();
        
        if self.stopped {
            tracing::debug!(block = %hash, "Insert failed: AEC is stopped");
            return false;
        }

        if self.cooldown.is_cooling_down() {
            tracing::debug!(
                block = %hash, 
                behavior = ?election_behavior,
                reason = ?self.cooldown.reason(),
                "Insert failed: AEC is cooling down"
            );
            // Don't return false here - we want to allow elections to start even during cooldown
        }

        // Check if we've reached max elections and election_behavior isn't high priority
        if self.roots.len() >= self.max_elections && 
           election_behavior != ElectionBehavior::Priority && 
           election_behavior != ElectionBehavior::Manual {
            tracing::debug!(
                block = %hash, 
                behavior = ?election_behavior,
                size = self.roots.len(),
                max = self.max_elections,
                "Insert failed: Max elections reached"
            );
            return false;
        }

        let root = block.qualified_root();

        if self.recently_confirmed.root_exists(&root) {
            // This block or a fork got recently confirmed, so there is no need for a new election.
            tracing::debug!(block = %hash, "Insert failed: Root exists in recently_confirmed");
            return false;
        }

        let existing = self.roots.get_mut(&root).map(|i| &mut i.election);

        if let Some(existing) = existing {
            // Try upgrading to priority election to enable immediate vote broadcasting.
            let previous_behavior = existing.behavior();
            let upgraded = existing.maybe_upgrade_to(election_behavior);
            if upgraded {
                *self.count_by_behavior_mut(previous_behavior) -= 1;
                *self.count_by_behavior_mut(election_behavior) += 1;
                tracing::debug!(
                    block = %hash, 
                    from = ?previous_behavior,
                    to = ?election_behavior,
                    "Election behavior upgraded"
                );
            } else {
                tracing::debug!(
                    block = %hash, 
                    existing = ?previous_behavior,
                    requested = ?election_behavior,
                    "Insert failed: Election already exists for this root"
                );
            }
            return false;
        }

        let election = Election::new(block, election_behavior, self.base_latency, now);

        self.roots.insert(Entry {
            root: root.clone(),
            election,
            erased_callback,
        });

        // Keep track of election count by election type
        *self.count_by_behavior_mut(election_behavior) += 1;
        self.vote_router.connect(hash, root);
        
        tracing::debug!(
            block = %hash, 
            behavior = ?election_behavior,
            count = %self.roots.len(),
            "Election inserted successfully"
        );
        
        true
    }

    pub(super) fn try_add_fork(&mut self, fork: &Block, fork_tally: Amount) -> AddForkResult {
        let Some(entry) = self.roots.get_mut(&fork.qualified_root()) else {
            return AddForkResult::ElectionEnded;
        };

        let result = entry.election.try_add_fork(fork, fork_tally);
        let added = match &result {
            AddForkResult::Added => true,
            AddForkResult::Replaced(removed) => {
                self.vote_router.disconnect(&removed.hash());
                true
            }
            AddForkResult::Duplicate => false,
            AddForkResult::TallyTooLow | AddForkResult::ElectionEnded => false,
        };

        if added {
            self.vote_router.connect(fork.hash(), fork.qualified_root());
        }

        result
    }

    /// How many election slots are available
    /// This is a soft limit and can be negative!
    pub fn vacancy(&self) -> i64 {
        if self.cooldown.is_cooling_down() {
            return 0;
        }
        let current_size = self.roots.len() as i64;
        self.max_elections as i64 - current_size
    }

    pub(super) fn set_cooldown(&mut self, cooldown_source: AecCooldownReason, cool_down: bool) {
        self.cooldown.set_cooldown(cooldown_source, cool_down);
    }

    pub(super) fn stop(&mut self) {
        self.stopped = true;
        self.roots.clear();
    }

    pub fn is_active_root(&self, root: &QualifiedRoot) -> bool {
        self.roots.get(root).is_some()
    }

    pub fn is_active_hash(&self, block_hash: &BlockHash) -> bool {
        self.vote_router.is_active(block_hash)
    }

    pub fn was_recently_confirmed(&self, block_hash: &BlockHash) -> bool {
        self.recently_confirmed.hash_exists(block_hash)
    }

    pub fn clear_recently_confirmed(&mut self) {
        self.recently_confirmed.clear();
    }

    pub fn transition_time(&mut self, now: Timestamp) {
        for entry in self.roots.iter_mut() {
            entry.election.transition_time(now);
        }
    }

    pub fn election_for_root(&self, root: &QualifiedRoot) -> Option<&Election> {
        self.roots.get(root).map(|i| &i.election)
    }

    pub fn election_for_root_mut(&mut self, root: &QualifiedRoot) -> Option<&mut Election> {
        self.roots.get_mut(root).map(|i| &mut i.election)
    }

    pub fn election_for_block(&self, block_hash: &BlockHash) -> Option<&Election> {
        let root = self.vote_router.qualified_root(block_hash)?;
        self.election_for_root(&root)
    }

    pub(super) fn election_for_block_mut(
        &mut self,
        block_hash: &BlockHash,
    ) -> Option<&mut Election> {
        let root = self.vote_router.qualified_root(block_hash)?;
        self.roots.get_mut(&root).map(|i| &mut i.election)
    }

    pub fn info(&self) -> ActiveElectionsInfo {
        ActiveElectionsInfo {
            max_elections: self.max_elections,
            total: self.roots.len(),
            priority: self.priority_count,
            hinted: self.hinted_count,
            optimistic: self.optimistic_count,
        }
    }

    pub(crate) fn erase_ended_elections(&mut self) -> Vec<Entry> {
        let removed = self.roots.drain_filter(|i| i.election.state().has_ended());

        for entry in &removed {
            self.cleanup_election(entry);
        }

        removed
    }

    pub(super) fn erase(&mut self, root: &QualifiedRoot) -> Option<Entry> {
        let entry = self.roots.erase(root);
        if let Some(e) = entry {
            self.cleanup_election(&e);
            Some(e)
        } else {
            None
        }
    }

    fn cleanup_election(&mut self, entry: &Entry) {
        let election = &entry.election;

        // Keep track of election count by election type
        *self.count_by_behavior_mut(election.behavior()) -= 1;
        self.vote_router.disconnect_election(&election);
        let winner_hash = election.winner().hash();
        if election.is_confirmed() {
            self.recently_confirmed
                .put(election.qualified_root().clone(), winner_hash);
        }
    }

    pub fn confirm_dependent_elections(
        &mut self,
        confirmed_blocks: Vec<(SavedBlock, Option<ConfirmedElection>)>,
        now: Timestamp,
    ) -> Vec<ConfirmedElection> {
        let mut results = Vec::new();

        for (block, source_election) in confirmed_blocks {
            let mut dependent_election = self.roots.get_mut(&block.qualified_root());

            // Distinguishes replay votes, cannot be determined if the block is not in any election
            // Dependent elections are implicitly confirmed when their block is cemented
            if let Some(election) = &mut dependent_election {
                // TRY CONFIRM
                // TODO: This should either confirm or cancel the election
                let winner_hash = election.election.winner().hash();
                if winner_hash == block.hash() {
                    election.election.force_confirm();
                }
            }

            // Check if the currently confirmed block was part of an election that triggered the confirmation
            if let Some(source) = source_election {
                if source.winner.hash() == block.hash() {
                    // This is the block that was directly confirmed by the source election.
                    // The election is already confirmed, so there is nothing to do.
                    continue;
                }
            }

            let mut confirmed_election = ConfirmedElection::new(block.clone());
            let mut handled = false;
            if let Some(dep_el) = dependent_election {
                if dep_el.election.is_confirmed() {
                    confirmed_election = dep_el
                        .election
                        .into_confirmed_election(now, ElectionResult::ActiveConfirmationHeight);
                    handled = true;
                }
            }

            if !handled {
                confirmed_election.result = ElectionResult::InactiveConfirmationHeight;
            }

            results.push(confirmed_election);
        }
        results
    }

    pub(super) fn remove_recently_confirmed(&mut self, block_hash: &BlockHash) {
        self.recently_confirmed.erase(block_hash);
    }

    pub fn len(&self) -> usize {
        self.roots.len()
    }

    pub fn container_info(&self) -> ContainerInfo {
        ContainerInfo::builder()
            .leaf("roots", self.roots.len(), RootContainer::ELEMENT_SIZE)
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
                self.recently_confirmed.container_info(),
            )
            .node("vote_router", self.vote_router.container_info())
            .finish()
    }

    /// Calculates minimum time delay between subsequent votes when processing non-final votes
    pub fn cooldown_time(rep_weight: Amount, online_weight: Amount) -> Duration {
        if rep_weight > online_weight / 20 {
            // Reps with more than 5% weight
            Duration::from_secs(1)
        } else if rep_weight > online_weight / 100 {
            // Reps with more than 1% weight
            Duration::from_secs(5)
        } else {
            // The rest of smaller reps
            Duration::from_secs(15)
        }
    }

    pub fn apply_votes(
        &mut self,
        votes: impl IntoIterator<Item = VoteSummary>,
        source: VoteSource,
        rep_weights: &HashMap<PublicKey, Amount>,
        online_weight: Amount,
        quorum_delta: Amount,
        now: Timestamp,
    ) -> (HashMap<BlockHash, VoteCode>, Vec<AecEvent>) {
        let mut results = HashMap::new();
        let mut events = Vec::new();

        for vote_summary in votes {
            // Ignore duplicate hashes (should not happen with a well-behaved voting node)
            if results.contains_key(&vote_summary.hash) {
                continue;
            }

            let root = self.vote_router.qualified_root(&vote_summary.hash);
            if let Some(root) = root {
                let entry = self.roots.get_mut(&root).unwrap();
                let election = &mut entry.election;

                let mut vote_code = VoteCode::Invalid;
                let rep_weight = rep_weights
                    .get(&vote_summary.voter)
                    .cloned()
                    .unwrap_or_default();

                if vote_code == VoteCode::Invalid {
                    if let Some(last_vote) = election.votes().get(&vote_summary.voter) {
                        if last_vote.timestamp > vote_summary.timestamp {
                            vote_code = VoteCode::Replay;
                        } else if last_vote.timestamp == vote_summary.timestamp
                            && !(last_vote.hash < vote_summary.hash)
                        {
                            vote_code = VoteCode::Replay;
                        }

                        if vote_code == VoteCode::Invalid {
                            let max_vote = vote_summary.timestamp == UnixMillisTimestamp::MAX
                                && last_vote.timestamp < vote_summary.timestamp;

                            let mut past_cooldown = true;
                            // Only cooldown live votes
                            if source != VoteSource::Cache {
                                let cooldown = ActiveElectionsContainer::cooldown_time(
                                    rep_weight,
                                    online_weight,
                                );
                                past_cooldown = last_vote.time <= SystemTime::now() - cooldown;
                            }

                            if !max_vote && !past_cooldown {
                                vote_code = VoteCode::Ignored;
                            }
                        }
                    }

                    if vote_code == VoteCode::Invalid {
                        election.add_vote(
                            vote_summary.voter,
                            vote_summary.timestamp,
                            vote_summary.hash,
                        );

                        events.push(AecEvent::VoteCounted(vote_summary.voter, source));

                        // CONFIRM IF QUORUM:
                        if !election.is_confirmed() {
                            let old_winner = election.winner().hash();
                            let old_final = election.is_final();

                            election.update_tallies(&rep_weights, quorum_delta);

                            let winner_changed = election.winner().hash() != old_winner;
                            if winner_changed {
                                events.push(AecEvent::WinnerChanged(
                                    old_winner,
                                    election.winner().deref().clone(),
                                ));
                            }

                            if election.is_final() {
                                if !old_final {
                                    events.push(AecEvent::FinalPhaseStarted(
                                        election.winner().hash(),
                                        election.qualified_root().clone(),
                                    ));
                                }
                                if election.is_confirmed() {
                                    let confirmed_election = election.into_confirmed_election(
                                        now,
                                        ElectionResult::ActiveConfirmedQuorum,
                                    );
                                    events.push(AecEvent::ElectionConfirmed(confirmed_election));
                                }
                            }
                        }

                        vote_code = VoteCode::Vote;
                    }
                }

                results.insert(vote_summary.hash, vote_code);
            } else {
                if !self.was_recently_confirmed(&vote_summary.hash) {
                    results.insert(vote_summary.hash, VoteCode::Indeterminate);
                } else {
                    results.insert(vote_summary.hash, VoteCode::Replay);
                }
            }
        }

        (results, events)
    }

    pub fn force_confirm(&mut self, block_hash: &BlockHash, now: Timestamp) -> Option<AecEvent> {
        let root = self.vote_router.qualified_root(block_hash)?;
        let entry = self.roots.get_mut(&root)?;
        let election = &mut entry.election;
        if election.force_confirm() {
            let confirmed_election =
                election.into_confirmed_election(now, ElectionResult::ActiveConfirmedQuorum);
            Some(AecEvent::ElectionConfirmed(confirmed_election))
        } else {
            None
        }
    }

    pub fn cancel(&mut self, root: &QualifiedRoot) {
        if let Some(entry) = self.roots.get_mut(root) {
            entry.election.cancel();
        }
    }
}

#[derive(Default)]
pub struct ActiveElectionsInfo {
    pub max_elections: usize,
    pub total: usize,
    pub priority: usize,
    pub hinted: usize,
    pub optimistic: usize,
}
