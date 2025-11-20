use std::sync::{Arc, RwLock};

use crate::{
    ConsensusServices,
    config::{NodeConfig, NodeFlags},
    block_processing::{BlockProcessor, BlockProcessorQueue},
    cementation::ConfirmingSet,
    consensus::{ActiveElectionsContainer, RequestAggregator, VoteGenerators, VoteProcessor, VoteProcessorQueue},
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
