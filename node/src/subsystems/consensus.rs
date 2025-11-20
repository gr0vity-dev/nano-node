use std::sync::{Arc, Mutex, RwLock};

use crate::{
    ConsensusServices,
    bootstrap::Bootstrapper,
    config::{NodeConfig, NodeFlags},
    block_processing::{BlockProcessor, BlockProcessorQueue, BoundedBacklog, LocalBlockBroadcaster},
    cementation::ConfirmingSet,
    consensus::{
        ActiveElectionsContainer, CurrentRepTiers, LocalVoteHistory, RequestAggregator,
        VoteCache, VoteCacheProcessor, VoteGenerators, VoteProcessor, VoteProcessorQueue,
        VoteRebroadcaster, WinnerBlockBroadcaster,
        election_schedulers::ElectionSchedulers,
    },
    representatives::{OnlineReps, RepCrawler},
};

use super::lifecycle::Lifecycle;
use std::ops::Deref;

/// Facade over consensus internals (active elections, vote processor, schedulers).
#[derive(Clone)]
pub struct ConsensusSubsystem {
    services: ConsensusServices,
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
    pub fn new(services: ConsensusServices, config: NodeConfig, flags: NodeFlags) -> Self {
        Self {
            services,
            config,
            flags,
        }
    }

    pub fn block_processor_queue(&self) -> Arc<BlockProcessorQueue> {
        self.services.block_processor_queue.clone()
    }

    pub fn active(&self) -> Arc<RwLock<ActiveElectionsContainer>> {
        self.services.active.clone()
    }

    pub fn confirming_set(&self) -> Arc<ConfirmingSet> {
        self.services.confirming_set.clone()
    }

    pub fn request_aggregator(&self) -> Arc<RequestAggregator> {
        self.services.request_aggregator.clone()
    }

    pub fn vote_processor_queue(&self) -> Arc<VoteProcessorQueue> {
        self.services.vote_processor_queue.clone()
    }

    pub fn vote_processor(&self) -> Arc<VoteProcessor> {
        self.services.vote_processor.clone()
    }

    pub fn vote_generators(&self) -> Arc<VoteGenerators> {
        self.services.vote_generators.clone()
    }

    pub fn block_processor(&self) -> Arc<BlockProcessor> {
        self.services.block_processor.clone()
    }

    pub fn services(&self) -> ConsensusServices {
        self.services.clone()
    }

    pub fn test_handles(&self) -> ConsensusTestHandles {
        ConsensusTestHandles {
            active: self.services.active.clone(),
            election_schedulers: self.services.election_schedulers.clone(),
            vote_processor: self.services.vote_processor.clone(),
            vote_generators: self.services.vote_generators.clone(),
            vote_history: self.services.vote_history.clone(),
            request_aggregator: self.services.request_aggregator.clone(),
            bounded_backlog: self.services.bounded_backlog.clone(),
            bootstrapper: self.services.bootstrapper.clone(),
            rep_crawler: self.services.rep_crawler.clone(),
            online_reps: self.services.online_reps.clone(),
            rep_tiers: self.services.rep_tiers.clone(),
            local_block_broadcaster: self.services.local_block_broadcaster.clone(),
            winner_block_broadcaster: self.services.winner_block_broadcaster.clone(),
            vote_processor_queue: self.services.vote_processor_queue.clone(),
            vote_cache: self.services.vote_cache.clone(),
            vote_cache_processor: self.services.vote_cache_processor.clone(),
            confirming_set: self.services.confirming_set.clone(),
            block_processor: self.services.block_processor.clone(),
            block_processor_queue: self.services.block_processor_queue.clone(),
            vote_rebroadcaster: self.services.vote_rebroadcaster.clone(),
        }
    }
}

impl Lifecycle for ConsensusSubsystem {
    fn start(&mut self) {
        self.services.start(&self.config, &self.flags);
    }

    fn stop(&mut self) {
        self.services.stop();
    }
}

impl Deref for ConsensusSubsystem {
    type Target = ConsensusServices;

    fn deref(&self) -> &Self::Target {
        &self.services
    }
}
