use std::sync::{Arc, Mutex, RwLock};

use crate::{
    block_processing::{
        BlockProcessor, BlockProcessorQueue, BoundedBacklog, LocalBlockBroadcaster,
        LocalBlockBroadcasterExt,
    },
    bootstrap::Bootstrapper,
    cementation::ConfirmingSet,
    config::{NodeConfig, NodeFlags},
    consensus::{
        ActiveElectionsContainer, CurrentRepTiers, LocalVoteHistory, RequestAggregator, VoteCache,
        VoteCacheProcessor, VoteGenerators, VoteProcessor, VoteProcessorExt, VoteProcessorQueue,
        VoteRebroadcaster, WinnerBlockBroadcaster, election_schedulers::ElectionSchedulers,
    },
    representatives::{OnlineReps, RepCrawler, RepCrawlerExt},
};

use super::lifecycle::Lifecycle;

/// Construction-only bundle of consensus collaborators used to wire up the
/// `ConsensusSubsystem`. This is strictly for composition; callers must not
/// store it on long-lived structs.
#[derive(Clone)]
pub struct ConsensusWiring {
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
}

/// Test-only access to consensus internals.
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
    pub fn new(wiring: ConsensusWiring, config: NodeConfig, flags: NodeFlags) -> Self {
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
        }
    }

    pub fn block_processor_queue(&self) -> Arc<BlockProcessorQueue> {
        self.block_processor_queue.clone()
    }

    pub fn active(&self) -> Arc<RwLock<ActiveElectionsContainer>> {
        self.active.clone()
    }

    pub fn confirming_set(&self) -> Arc<ConfirmingSet> {
        self.confirming_set.clone()
    }

    pub fn request_aggregator(&self) -> Arc<RequestAggregator> {
        self.request_aggregator.clone()
    }

    pub fn vote_processor_queue(&self) -> Arc<VoteProcessorQueue> {
        self.vote_processor_queue.clone()
    }

    pub fn vote_processor(&self) -> Arc<VoteProcessor> {
        self.vote_processor.clone()
    }

    pub fn vote_generators(&self) -> Arc<VoteGenerators> {
        self.vote_generators.clone()
    }

    pub fn block_processor(&self) -> Arc<BlockProcessor> {
        self.block_processor.clone()
    }

    pub fn election_schedulers(&self) -> Arc<ElectionSchedulers> {
        self.election_schedulers.clone()
    }

    pub fn online_reps(&self) -> Arc<Mutex<OnlineReps>> {
        self.online_reps.clone()
    }

    pub fn rep_tiers(&self) -> Arc<CurrentRepTiers> {
        self.rep_tiers.clone()
    }

    fn start_internal(&self) {
        if self.config.enable_vote_processor {
            self.vote_processor.start();
        }
        self.block_processor.start(self.config.block_processor_threads);
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
