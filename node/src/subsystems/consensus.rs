//! ConsensusSubsystem coordinates election scheduling, vote processing, and confirmation. Production APIs expose lifecycle and aggregate info; raw internals are available only via test handles.
use std::{
    net::SocketAddrV6,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use anyhow::Result;
use crate::{
    block_processing::{
        BlockContext, BlockProcessor, BlockProcessorQueue, BlockSource, BoundedBacklog,
        LocalBlockBroadcaster, LocalBlockBroadcasterExt,
    },
    bootstrap::Bootstrapper,
    cementation::{ConfirmingSet, ConfirmingSetInfo},
    config::{NetworkParams, NodeConfig, NodeFlags},
    consensus::{
        ActiveElectionsContainer, ActiveElectionsInfo, AecTicker, AecVoter, CurrentRepTiers,
        FilteredVote, LocalVoteHistory, ReceivedVote, RepTier, RequestAggregator, VoteCache,
        VoteCacheProcessor, VoteGenerationEvent, VoteGenerators, VoteProcessor, VoteProcessorExt,
        VoteProcessorQueue, VoteRebroadcaster, WinnerBlockBroadcaster,
        election_schedulers::ElectionSchedulers,
    },
    representatives::{OnlineRepInfo, OnlineReps, PeeredRepInfo, RepCrawler, RepCrawlerExt},
};
use rsnano_ledger::BlockError;
#[cfg(any(test, feature = "test_support"))]
use rsnano_network::ChannelId;
use rsnano_nullable_clock::Timestamp;
use rsnano_output_tracker::OutputTrackerMt;
use rsnano_types::{
    Amount, Block, BlockHash, PublicKey, QualifiedRoot, Root, SavedBlock, UnixMillisTimestamp, Vote,
    VoteError, VoteSource,
};
use rsnano_utils::fair_queue::FairQueueInfo;
use rsnano_utils::ticker::TimerThread;

use super::lifecycle::Lifecycle;

/// Construction-only bundle of consensus collaborators used to wire up the
/// `ConsensusSubsystem`. This is strictly for composition; callers must not
/// store it on long-lived structs.
#[derive(Clone)]
pub(crate) struct ConsensusWiring {
    pub(crate) active: Arc<RwLock<ActiveElectionsContainer>>,
    pub(crate) election_schedulers: Arc<ElectionSchedulers>,
    pub(crate) vote_processor: Arc<VoteProcessor>,
    pub(crate) vote_generators: Arc<VoteGenerators>,
    pub(crate) vote_history: Arc<LocalVoteHistory>,
    pub(crate) request_aggregator: Arc<RequestAggregator>,
    pub(crate) bounded_backlog: Arc<BoundedBacklog>,
    pub(crate) bootstrapper: Arc<Bootstrapper>,
    pub(crate) rep_crawler: Arc<RepCrawler>,
    pub(crate) online_reps: Arc<Mutex<OnlineReps>>,
    pub(crate) rep_tiers: Arc<CurrentRepTiers>,
    pub(crate) local_block_broadcaster: Arc<LocalBlockBroadcaster>,
    pub(crate) winner_block_broadcaster: Arc<Mutex<WinnerBlockBroadcaster>>,
    pub(crate) vote_processor_queue: Arc<VoteProcessorQueue>,
    pub(crate) vote_cache: Arc<Mutex<VoteCache>>,
    pub(crate) vote_cache_processor: Arc<VoteCacheProcessor>,
    pub(crate) confirming_set: Arc<ConfirmingSet>,
    pub(crate) block_processor: Arc<BlockProcessor>,
    pub(crate) block_processor_queue: Arc<BlockProcessorQueue>,
    pub(crate) vote_rebroadcaster: Arc<Mutex<VoteRebroadcaster>>,
}

/// Runtime context for consensus execution and timers.
#[derive(Clone)]
pub(crate) struct ConsensusContext {
    pub(crate) config: NodeConfig,
    pub(crate) flags: NodeFlags,
    pub(crate) network_params: NetworkParams,
    pub(crate) aec_ticker: Arc<TimerThread<AecTicker>>,
    pub(crate) aec_voter: Arc<TimerThread<AecVoter>>,
}

#[derive(Clone)]
pub struct OnlineRepsSnapshot {
    pub quorum_delta: Amount,
    pub quorum_percent: u8,
    pub online_weight_minimum: Amount,
    pub online_weight: Amount,
    pub trended_weight: Amount,
    pub peered_weight: Amount,
    pub minimum_principal_weight: Amount,
    pub peered_reps: Vec<PeeredRepInfo>,
    pub online_reps: Vec<OnlineRepInfo>,
}

#[derive(Clone)]
pub struct RequestAggregatorInfo {
    pub is_empty: bool,
    pub queue_len: usize,
}

#[derive(Clone)]
pub struct VoteByAccountView {
    pub account: PublicKey,
    pub hash: BlockHash,
    pub weight: Amount,
    pub vote_created: UnixMillisTimestamp,
    pub vote_received: Timestamp,
    pub is_final: bool,
}

#[derive(Clone)]
pub struct ActiveElectionView {
    pub has_max_blocks: bool,
    pub block_count: usize,
    pub vote_count: usize,
    pub is_confirmed: bool,
    pub winner_hash: BlockHash,
    pub candidate_hashes: Vec<BlockHash>,
    pub votes_by_account: Vec<VoteByAccountView>,
}

#[derive(Clone)]
pub struct VoteCacheVoterView {
    pub representative: PublicKey,
    pub weight: Amount,
    pub timestamp: UnixMillisTimestamp,
    pub is_final: bool,
    pub vote_hashes: Vec<BlockHash>,
}

#[derive(Clone)]
pub struct VoteCacheEntryView {
    pub hash: BlockHash,
    pub tally: Amount,
    pub final_tally: Amount,
    pub voters: Vec<VoteCacheVoterView>,
}

#[derive(Clone)]
pub struct VoteCacheView {
    pub size: usize,
    pub entries: Vec<VoteCacheEntryView>,
}

#[derive(Clone)]
pub struct VoteHistoryEntry {
    pub id: usize,
    pub vote: Vote,
}

/// Facade over consensus internals (active elections, vote processor, schedulers).
#[derive(Clone)]
pub struct ConsensusSubsystem {
    active: Arc<RwLock<ActiveElectionsContainer>>,
    election_schedulers: Arc<ElectionSchedulers>,
    vote_processor: Arc<VoteProcessor>,
    vote_generators: Arc<VoteGenerators>,
    vote_history: Arc<LocalVoteHistory>,
    request_aggregator: Arc<RequestAggregator>,
    bounded_backlog: Arc<BoundedBacklog>,
    bootstrapper: Arc<Bootstrapper>,
    rep_crawler: Arc<RepCrawler>,
    online_reps: Arc<Mutex<OnlineReps>>,
    rep_tiers: Arc<CurrentRepTiers>,
    local_block_broadcaster: Arc<LocalBlockBroadcaster>,
    winner_block_broadcaster: Arc<Mutex<WinnerBlockBroadcaster>>,
    vote_processor_queue: Arc<VoteProcessorQueue>,
    vote_cache: Arc<Mutex<VoteCache>>,
    vote_cache_processor: Arc<VoteCacheProcessor>,
    confirming_set: Arc<ConfirmingSet>,
    block_processor: Arc<BlockProcessor>,
    block_processor_queue: Arc<BlockProcessorQueue>,
    vote_rebroadcaster: Arc<Mutex<VoteRebroadcaster>>,
    config: NodeConfig,
    flags: NodeFlags,
    network_params: NetworkParams,
    aec_ticker: Arc<TimerThread<AecTicker>>,
    aec_voter: Arc<TimerThread<AecVoter>>,
}

/// Test-only access to consensus internals.
#[cfg(any(test, feature = "test_support"))]
#[allow(private_interfaces)]
#[derive(Clone)]
pub struct ConsensusTestHandles {
    pub active: Arc<RwLock<ActiveElectionsContainer>>,
    pub election_schedulers: Arc<ElectionSchedulers>,
    pub vote_processor: Arc<VoteProcessor>,
    pub vote_generators: Arc<VoteGenerators>,
    pub vote_history: Arc<LocalVoteHistory>,
    pub request_aggregator: Arc<RequestAggregator>,
    pub bounded_backlog: Arc<BoundedBacklog>,
    pub bootstrapper: Arc<Bootstrapper>,
    pub rep_crawler: Arc<RepCrawler>,
    pub online_reps: Arc<Mutex<OnlineReps>>,
    pub rep_tiers: Arc<CurrentRepTiers>,
    pub local_block_broadcaster: Arc<LocalBlockBroadcaster>,
    pub winner_block_broadcaster: Arc<Mutex<WinnerBlockBroadcaster>>,
    pub vote_processor_queue: Arc<VoteProcessorQueue>,
    pub vote_cache: Arc<Mutex<VoteCache>>,
    pub vote_cache_processor: Arc<VoteCacheProcessor>,
    pub confirming_set: Arc<ConfirmingSet>,
    pub block_processor: Arc<BlockProcessor>,
    pub block_processor_queue: Arc<BlockProcessorQueue>,
    pub vote_rebroadcaster: Arc<Mutex<VoteRebroadcaster>>,
}

impl ConsensusSubsystem {
    pub(crate) fn new(wiring: ConsensusWiring, context: ConsensusContext) -> Self {
        let ConsensusWiring {
            active,
            election_schedulers,
            vote_processor,
            vote_generators,
            vote_history,
            request_aggregator,
            bounded_backlog,
            bootstrapper,
            rep_crawler,
            online_reps,
            rep_tiers,
            local_block_broadcaster,
            winner_block_broadcaster,
            vote_processor_queue,
            vote_cache,
            vote_cache_processor,
            confirming_set,
            block_processor,
            block_processor_queue,
            vote_rebroadcaster,
        } = wiring;
        let ConsensusContext {
            config,
            flags,
            network_params,
            aec_ticker,
            aec_voter,
        } = context;

        Self {
            active,
            election_schedulers,
            vote_processor,
            vote_generators,
            vote_history,
            request_aggregator,
            bounded_backlog,
            bootstrapper,
            rep_crawler,
            online_reps,
            rep_tiers,
            local_block_broadcaster,
            winner_block_broadcaster,
            vote_processor_queue,
            vote_cache,
            vote_cache_processor,
            confirming_set,
            block_processor,
            block_processor_queue,
            vote_rebroadcaster,
            config,
            flags,
            network_params,
            aec_ticker,
            aec_voter,
        }
    }

    pub fn enqueue_block(&self, context: BlockContext) {
        self.block_processor_queue.push(context);
    }

    pub fn push_block_blocking(&self, block: Block, source: BlockSource) -> Result<(), BlockError> {
        self.block_processor_queue
            .push_blocking(Arc::new(block), source)
            .map_err(|_| BlockError::BadSignature)?
            .map(|_| ())
    }

    pub fn block_processor_queue_info(&self) -> FairQueueInfo<BlockSource> {
        self.block_processor_queue.info()
    }

    pub fn vote_processor_queue_info(&self) -> FairQueueInfo<RepTier> {
        self.vote_processor_queue.info()
    }

    pub fn active_info(&self) -> ActiveElectionsInfo {
        self.with_active(|active| active.info())
    }

    pub fn scheduler_limits(&self) -> (usize, usize) {
        (
            self.election_schedulers.optimistic.max_elections,
            self.election_schedulers.hinted.max_elections,
        )
    }

    pub fn push_manual(&self, block: SavedBlock) {
        self.election_schedulers.manual.push(block);
    }

    pub fn confirming_set_info(&self) -> ConfirmingSetInfo {
        self.confirming_set.info()
    }

    pub fn with_active<R>(&self, f: impl FnOnce(&ActiveElectionsContainer) -> R) -> R {
        let guard = self.active.read().unwrap();
        f(&guard)
    }

    pub fn with_active_mut<R>(&self, f: impl FnOnce(&mut ActiveElectionsContainer) -> R) -> R {
        let mut guard = self.active.write().unwrap();
        f(&mut guard)
    }

    pub fn is_active_root(&self, root: &QualifiedRoot) -> bool {
        self.with_active(|active| active.is_active_root(root))
    }

    pub fn is_active_hash(&self, hash: &BlockHash) -> bool {
        self.with_active(|active| active.is_active_hash(hash))
    }

    pub fn erase_active(&self, root: &QualifiedRoot) -> bool {
        self.with_active_mut(|active| active.erase(root))
    }

    pub fn force_confirm(&self, block_hash: &BlockHash, now: Timestamp) {
        self.with_active_mut(|active| active.force_confirm(block_hash, now));
    }

    pub fn online_reps_snapshot(&self) -> OnlineRepsSnapshot {
        let reps = self.online_reps.lock().unwrap();
        OnlineRepsSnapshot {
            quorum_delta: reps.quorum_delta(),
            quorum_percent: reps.quorum_percent(),
            online_weight_minimum: reps.online_weight_minimum(),
            online_weight: reps.online_weight(),
            trended_weight: reps.trended_or_minimum_weight(),
            peered_weight: reps.peered_weight(),
            minimum_principal_weight: reps.minimum_principal_weight(),
            peered_reps: reps.peered_reps(),
            online_reps: reps.online_reps().collect(),
        }
    }

    pub fn request_aggregator_info(&self) -> RequestAggregatorInfo {
        RequestAggregatorInfo {
            is_empty: self.request_aggregator.is_empty(),
            queue_len: self.request_aggregator.len(),
        }
    }

    pub fn inject_vote(
        &self,
        vote: Vote,
        source: VoteSource,
        endpoint: Option<SocketAddrV6>,
    ) -> Result<()> {
        let _ = endpoint;
        let queued = self
            .vote_processor_queue
            .enqueue(Arc::new(vote), None, source, None);
        if queued {
            Ok(())
        } else {
            Err(anyhow!("vote queue overfilled"))
        }
    }

    pub fn process_vote_blocking(
        &self,
        vote: Vote,
        source: VoteSource,
        endpoint: Option<SocketAddrV6>,
    ) -> Result<(), VoteError> {
        let _ = endpoint;
        let received = ReceivedVote::new(Arc::new(vote), source, None);
        let filtered: FilteredVote = received.into();
        self.vote_processor.vote_blocking(&filtered)
    }

    pub fn broadcast_block_initial(&self, block: Arc<Block>) {
        self.local_block_broadcaster
            .flood_block_initial((*block).clone());
    }

    pub fn active_election_snapshot(
        &self,
        root: QualifiedRoot,
    ) -> Option<ActiveElectionView> {
        self.with_active(|active| {
            active.election_for_root(&root).map(|election| {
                let votes_by_account = election
                    .votes()
                    .values()
                    .map(|summary| VoteByAccountView {
                        account: summary.voter,
                        hash: summary.hash,
                        weight: summary.weight,
                        vote_created: summary.vote_created,
                        vote_received: summary.vote_received,
                        is_final: summary.is_final_vote(),
                    })
                    .collect();

                ActiveElectionView {
                    has_max_blocks: election.has_max_blocks(),
                    block_count: election.block_count(),
                    vote_count: election.vote_count(),
                    is_confirmed: election.is_confirmed(),
                    winner_hash: election.winner().hash(),
                    candidate_hashes: election
                        .candidate_blocks()
                        .keys()
                        .cloned()
                        .collect(),
                    votes_by_account,
                }
            })
        })
    }

    pub fn vote_cache_snapshot(&self) -> VoteCacheView {
        let cache = self.vote_cache.lock().unwrap();
        let entries = cache
            .entries()
            .map(|entry| {
                let voters = entry
                    .voters
                    .iter_unordered()
                    .map(|voter| VoteCacheVoterView {
                        representative: voter.representative,
                        weight: voter.weight,
                        timestamp: voter.vote.timestamp(),
                        is_final: voter.vote.is_final(),
                        vote_hashes: voter.vote.hashes.clone(),
                    })
                    .collect();

                VoteCacheEntryView {
                    hash: entry.hash,
                    tally: entry.tally(),
                    final_tally: entry.final_tally(),
                    voters,
                }
            })
            .collect();

        VoteCacheView {
            size: cache.size(),
            entries,
        }
    }

    pub fn vote_history_snapshot(
        &self,
        root: &Root,
        hash: &BlockHash,
        is_final: bool,
    ) -> Vec<VoteHistoryEntry> {
        self.vote_history
            .votes(root, hash, is_final)
            .into_iter()
            .map(|vote| VoteHistoryEntry {
                id: Arc::as_ptr(&vote) as usize,
                vote: (*vote).clone(),
            })
            .collect()
    }

    pub fn track_vote_generation(&self) -> Arc<OutputTrackerMt<VoteGenerationEvent>> {
        self.vote_generators.track()
    }

    #[cfg(any(test, feature = "test_support"))]
    pub fn process_rep_crawler_vote(&self, vote: Vote, channel_id: ChannelId) {
        self.rep_crawler.force_process_vote(vote, channel_id);
    }

    #[cfg(any(test, feature = "test_support"))]
    #[doc(hidden)]
    pub fn aec_ticker(&self) -> Arc<TimerThread<AecTicker>> {
        self.aec_ticker.clone()
    }

    fn start_internal(&self) {
        self.aec_voter.start(Duration::from_millis(20));
        if !self.flags.disable_request_loop {
            self.aec_ticker
                .start(self.network_params.network.aec_loop_interval);
        }
        if self.config.enable_vote_processor {
            self.vote_processor.start();
        }
        self.block_processor
            .start(self.config.block_processor_threads);
        if !self.flags.disable_rep_crawler {
            self.rep_crawler.start();
        }
        self.vote_generators.start();
        self.request_aggregator.start();
        self.confirming_set.start();
        self.election_schedulers.start();
        if self.config.enable_bounded_backlog {
            self.bounded_backlog.start();
        }
        self.local_block_broadcaster.start();
        self.vote_cache_processor.start();
        if self.config.enable_vote_rebroadcast {
            self.vote_rebroadcaster.lock().unwrap().start();
        }
    }

    fn stop_internal(&self) {
        self.aec_ticker.stop();
        self.aec_voter.stop();
        self.local_block_broadcaster.stop();
        self.request_aggregator.stop();
        self.vote_processor.stop();
        self.election_schedulers.stop();
        self.active.write().unwrap().stop();
        self.vote_generators.stop();
        self.confirming_set.stop();
        self.bounded_backlog.stop();
        self.rep_crawler.stop();
        self.block_processor.stop();
        self.vote_rebroadcaster.lock().unwrap().stop();
        self.vote_cache_processor.stop();
    }

    /// **Legacy test access - technical debt.**
    ///
    /// This method exposes internal subsystem components for testing.
    /// It is marked hidden and should be avoided in new tests.
    /// Phase 5 will introduce behavioral test helpers to replace this pattern.
    #[cfg(any(test, feature = "test_support"))]
    #[doc(hidden)]
    pub fn test_handles(&self) -> ConsensusTestHandles {
        ConsensusTestHandles {
            active: self.active.clone(),
            election_schedulers: self.election_schedulers.clone(),
            vote_processor: self.vote_processor.clone(),
            vote_generators: self.vote_generators.clone(),
            vote_history: self.vote_history.clone(),
            request_aggregator: self.request_aggregator.clone(),
            bounded_backlog: self.bounded_backlog.clone(),
            bootstrapper: self.bootstrapper.clone(),
            rep_crawler: self.rep_crawler.clone(),
            online_reps: self.online_reps.clone(),
            rep_tiers: self.rep_tiers.clone(),
            local_block_broadcaster: self.local_block_broadcaster.clone(),
            winner_block_broadcaster: self.winner_block_broadcaster.clone(),
            vote_processor_queue: self.vote_processor_queue.clone(),
            vote_cache: self.vote_cache.clone(),
            vote_cache_processor: self.vote_cache_processor.clone(),
            confirming_set: self.confirming_set.clone(),
            block_processor: self.block_processor.clone(),
            block_processor_queue: self.block_processor_queue.clone(),
            vote_rebroadcaster: self.vote_rebroadcaster.clone(),
        }
    }
}

impl Lifecycle for ConsensusSubsystem {
    fn start(&mut self) {
        self.start_internal();
    }

    fn stop(&mut self) {
        self.stop_internal();
    }
}
