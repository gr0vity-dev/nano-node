mod active_elections_container;
mod cooldown_controller;
mod recently_confirmed_cache;
mod root_container;

pub use active_elections_container::*;
pub use cooldown_controller::AecCooldownReason;
use rsnano_ledger::RepWeightCache;
use rsnano_network::Channel;

use std::{
    collections::HashMap,
    sync::{Arc, RwLock, RwLockReadGuard},
    time::{Duration, SystemTime},
};

use root_container::{Entry, RootContainer};
use rsnano_nullable_clock::SteadyClock;
use tracing::debug;

use rsnano_core::{
    utils::BackpressureSender, Amount, Block, BlockHash, PublicKey, QualifiedRoot, SavedBlock,
    Vote, VoteCode, VoteSource,
};
use rsnano_stats::{DetailType, Sample, StatType, Stats};

use super::{AddForkResult, ConfirmedElection, ElectionBehavior, VoteRouter, VoteSummary};

#[derive(Clone, Debug, PartialEq)]
pub struct ActiveElectionsConfig {
    /// Maximum number of simultaneous active elections (AEC size)
    pub max_elections: usize,
    /// Maximum cache size for recently_confirmed
    pub confirmation_cache: usize,
}

impl Default for ActiveElectionsConfig {
    fn default() -> Self {
        Self {
            max_elections: 5000,
            confirmation_cache: 65536,
        }
    }
}

#[derive(Clone)]
pub enum AecEvent {
    ElectionStarted(BlockHash),
    ElectionStopped(BlockHash),
    ElectionConfirmed(ConfirmedElection),

    BlockAddedToElection(BlockHash),
    BlockDiscarded(Block),
    BlockConfirmed(SavedBlock, ConfirmedElection),
    VoteCounted(PublicKey, VoteSource),
    /// old winner + new winner block
    WinnerChanged(BlockHash, Block),

    VoteProcessed(
        Arc<Vote>,
        Amount,
        VoteSource,
        Option<Arc<Channel>>,
        HashMap<BlockHash, VoteCode>,
    ),
    FinalPhaseStarted(BlockHash, QualifiedRoot),
    VacancyUpdated,
}

pub struct ActiveElections {
    container: RwLock<ActiveElectionsContainer>,
    max_elections: usize,
    stats: Arc<Stats>,
    clock: Arc<SteadyClock>,
    rep_weights: Arc<RepWeightCache>,
    event_sender: RwLock<Option<BackpressureSender<AecEvent>>>,
}

impl ActiveElections {
    pub(crate) fn new(
        config: ActiveElectionsConfig,
        stats: Arc<Stats>,
        rep_weights: Arc<RepWeightCache>,
        base_latency: Duration,
        clock: Arc<SteadyClock>,
    ) -> Self {
        let max_elections = config.max_elections;
        Self {
            container: RwLock::new(ActiveElectionsContainer::new(config, base_latency)),
            max_elections,
            rep_weights,
            stats,
            clock,
            event_sender: RwLock::new(None),
        }
    }

    pub fn set_event_sink(&self, sink: BackpressureSender<AecEvent>) {
        *self.event_sender.write().unwrap() = Some(sink);
    }

    pub fn read(&self) -> RwLockReadGuard<ActiveElectionsContainer> {
        self.container.read().unwrap()
    }

    pub fn len(&self) -> usize {
        self.container.read().unwrap().len()
    }

    pub fn max_len(&self) -> usize {
        self.max_elections
    }

    pub fn set_cooldown(&self, cooldown_source: AecCooldownReason, cool_down: bool) {
        self.container
            .write()
            .unwrap()
            .set_cooldown(cooldown_source, cool_down);
    }

    pub fn is_active_root(&self, root: &QualifiedRoot) -> bool {
        self.container.read().unwrap().is_active_root(root)
    }

    pub fn is_active_hash(&self, block_hash: &BlockHash) -> bool {
        self.container.read().unwrap().is_active_hash(block_hash)
    }

    pub fn clear_recently_confirmed(&self) {
        self.container.write().unwrap().clear_recently_confirmed();
    }

    pub fn transition_time(&self) {
        let now = self.clock.now();
        self.container.write().unwrap().transition_time(now);
    }

    pub fn erase_ended_elections(&self) {
        let erased = self.container.write().unwrap().erase_ended_elections();
        let something_erased = erased.len() > 0;
        for entry in erased {
            self.handle_removed_election(entry);
        }
        if something_erased {
            self.notify(AecEvent::VacancyUpdated);
        }
    }

    pub fn erase(&self, root: &QualifiedRoot) -> bool {
        let removed = self.container.write().unwrap().erase(root);
        let was_removed = removed.is_some();

        if let Some(entry) = removed {
            self.handle_removed_election(entry);
            self.notify(AecEvent::VacancyUpdated);
        }

        was_removed
    }

    fn handle_removed_election(&self, entry: Entry) {
        self.add_stats(&entry);

        let election = &entry.election;
        let winner_hash = election.winner().hash();
        let is_confirmed = election.is_confirmed();
        let blocks = election.candidate_blocks().clone();

        for (hash, block) in blocks {
            // Notify observers about dropped elections & blocks lost confirmed elections
            if !is_confirmed || hash != winner_hash {
                self.notify(AecEvent::ElectionStopped(hash));
            }

            if !is_confirmed {
                self.notify(AecEvent::BlockDiscarded(block.into()));
            }
        }
    }

    fn add_stats(&self, entry: &Entry) {
        let election = &entry.election;
        let is_confirmed = election.is_confirmed();
        let state = election.state();
        let behavior = election.behavior();
        let start = election.start();
        let root = election.qualified_root().clone();

        self.stats
            .inc(StatType::ActiveElections, DetailType::Stopped);

        self.stats.inc(
            StatType::ActiveElections,
            if is_confirmed {
                DetailType::Confirmed
            } else {
                DetailType::Unconfirmed
            },
        );
        self.stats
            .inc(StatType::ActiveElectionsStopped, state.into());
        self.stats.inc(state.into(), behavior.into());
        // Track election duration
        self.stats.sample(
            Sample::ActiveElectionDuration,
            start.elapsed(self.clock.now()).as_millis() as i64,
            (0, 1000 * 60 * 10),
        ); // 0-10 minutes range

        // Notify observers without holding the lock
        if let Some(callback) = &entry.erased_callback {
            callback(&root);
        }
    }

    pub fn insert(
        &self,
        block: SavedBlock,
        election_behavior: ElectionBehavior,
        erased_callback: Option<ErasedCallback>,
    ) -> bool {
        let hash = block.hash();

        let inserted = self.container.write().unwrap().insert(
            block,
            election_behavior,
            erased_callback,
            self.clock.now(),
        );

        // Add debug log to track if insert is failing
        debug!(inserted = inserted, block = %hash, behavior = ?election_behavior, "Election insert result");

        if inserted {
            self.stats
                .inc(StatType::ActiveElections, DetailType::Started);
            self.stats
                .inc(StatType::ActiveElectionsStarted, election_behavior.into());

            debug!(behavior = ?election_behavior, block = %hash, "Started new election");
            self.notify(AecEvent::ElectionStarted(hash));
        }

        inserted
    }

    pub fn try_add_fork(&self, fork: &Block, fork_tally: Amount) -> bool {
        let result = self
            .container
            .write()
            .unwrap()
            .try_add_fork(fork, fork_tally);

        let added = match result {
            AddForkResult::Added => {
                self.notify(AecEvent::BlockAddedToElection(fork.hash()));
                true
            }
            AddForkResult::Replaced(removed) => {
                self.notify(AecEvent::BlockDiscarded(removed.into()));
                self.notify(AecEvent::BlockAddedToElection(fork.hash()));
                true
            }
            AddForkResult::TallyTooLow => {
                self.notify(AecEvent::BlockDiscarded(fork.clone()));
                false
            }
            AddForkResult::Duplicate | AddForkResult::ElectionEnded => false,
        };

        added
    }

    pub fn remove_recently_confirmed(&self, block_hash: &BlockHash) {
        self.container
            .write()
            .unwrap()
            .remove_recently_confirmed(block_hash);
    }

    pub fn stop(&self) {
        self.container.write().unwrap().stop();
        // destroy send queue so that the receiver thread will be stopped too
        drop(self.event_sender.write().unwrap().take());
    }

    fn notify(&self, event: AecEvent) {
        if let Some(sender) = self.event_sender.read().unwrap().as_ref() {
            sender.send(event).unwrap()
        }
    }

    pub fn transition_active_hash(&self, block_hash: &BlockHash) -> bool {
        let mut container = self.container.write().unwrap();
        let Some(election) = container.election_for_block_mut(block_hash) else {
            return false;
        };
        election.transition_active();
        true
    }

    // TODO: Delete!
    pub fn transition_active(&self, root: &QualifiedRoot) {
        self.container
            .write()
            .unwrap()
            .election_for_root_mut(root)
            .unwrap()
            .transition_active();
    }

    // TODO: Delete!
    pub fn change_vote_timestamp(
        &self,
        root: &QualifiedRoot,
        voter: &PublicKey,
        new_timestamp: SystemTime,
    ) {
        self.container
            .write()
            .unwrap()
            .election_for_root_mut(root)
            .expect("No election found for given root")
            .change_vote_timestamp(voter, new_timestamp);
    }

    pub fn confirm_dependent_elections(
        &self,
        confirmed_blocks: Vec<(SavedBlock, Option<ConfirmedElection>)>,
    ) -> Vec<ConfirmedElection> {
        let now = self.clock.now();
        self.container
            .write()
            .unwrap()
            .confirm_dependent_elections(confirmed_blocks, now)
    }

    pub fn remove_votes<'a>(
        &self,
        root: &QualifiedRoot,
        voters: impl IntoIterator<Item = &'a PublicKey>,
    ) {
        let mut container = self.container.write().unwrap();
        let Some(election) = container.election_for_root_mut(root) else {
            return;
        };
        for voter in voters {
            election.remove_vote(voter);
        }
    }

    pub fn apply_votes(
        &self,
        votes: impl IntoIterator<Item = VoteSummary>,
        source: VoteSource,
        online_weight: Amount,
        quorum_delta: Amount,
    ) -> HashMap<BlockHash, VoteCode> {
        let (results, events) = {
            let mut container = self.container.write().unwrap();
            let rep_weights = self.rep_weights.read();
            container.apply_votes(
                votes,
                source,
                &rep_weights,
                online_weight,
                quorum_delta,
                self.clock.now(),
            )
        };

        for e in events {
            self.notify(e);
        }

        results
    }

    pub fn force_confirm(&self, block_hash: &BlockHash) {
        let event = self
            .container
            .write()
            .unwrap()
            .force_confirm(block_hash, self.clock.now());

        if let Some(e) = event {
            self.notify(e);
        } else {
            panic!("Force confirm failed, because no active election was found");
        }
    }

    pub fn cancel(&self, root: &QualifiedRoot) {
        self.container.write().unwrap().cancel(root);
    }
}

impl Drop for ActiveElections {
    fn drop(&mut self) {
        self.stop()
    }
}

pub(crate) type ErasedCallback = Box<dyn Fn(&QualifiedRoot) + Send + Sync>;

pub struct ApplyVoteResult {
    pub voted_block: BlockHash,
    pub vote_result: VoteCode,
    pub events: Vec<AecEvent>,
}

impl ApplyVoteResult {
    pub fn new(voted_block: BlockHash, vote_result: VoteCode) -> Self {
        Self {
            voted_block,
            vote_result,
            events: Vec::new(),
        }
    }
}
