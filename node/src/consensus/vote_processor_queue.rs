use std::{
    collections::VecDeque,
    mem::size_of,
    sync::{Arc, Condvar, Mutex},
};

use strum::IntoEnumIterator;

use rsnano_core::{
    utils::{ContainerInfo, ContainerInfoProvider, FairQueue, FairQueueInfo},
    BlockHash, Vote, VoteSource,
};
use rsnano_network::{Channel, ChannelId, DeadChannelCleanupStep};
use rsnano_stats::{DetailType, StatType, Stats};
use rsnano_stats::{StatsCollection, StatsSource};

use super::{RepTier, RepTiers, RepTiersConsumer, VoteProcessorConfig};

pub struct VoteProcessorQueue {
    data: Mutex<VoteProcessorQueueData>,
    condition: Condvar,
    pub config: VoteProcessorConfig,
    stats: Arc<Stats>,
}

impl VoteProcessorQueue {
    pub fn new(config: VoteProcessorConfig, stats: Arc<Stats>) -> Self {
        let conf = config.clone();
        Self {
            data: Mutex::new(VoteProcessorQueueData {
                stopped: false,
                rep_tiers: Default::default(),
                queue: FairQueue::new(
                    move |(tier, channel)| {
                        let max_size = match tier {
                            RepTier::Tier1 | RepTier::Tier2 | RepTier::Tier3 => conf.max_pr_queue,
                            RepTier::None => conf.max_non_pr_queue,
                        };
                        if *channel == ChannelId::LOOPBACK {
                            // allow more votes for LOOPBACK, which comes from the vote cache!
                            max_size * 10
                        } else {
                            max_size
                        }
                    },
                    move |(tier, _)| match tier {
                        RepTier::Tier3 => conf.pr_priority * conf.pr_priority * conf.pr_priority,
                        RepTier::Tier2 => conf.pr_priority * conf.pr_priority,
                        RepTier::Tier1 => conf.pr_priority,
                        RepTier::None => 1,
                    },
                ),
            }),
            condition: Condvar::new(),
            config,
            stats,
        }
    }

    pub fn len(&self) -> usize {
        self.data.lock().unwrap().queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.lock().unwrap().queue.is_empty()
    }

    /// Queue vote for processing. @returns true if the vote was queued
    pub fn enqueue(
        &self,
        vote: Arc<Vote>,
        channel: Option<Arc<Channel>>,
        source: VoteSource,
        filter: Option<BlockHash>,
    ) -> bool {
        let channel_id = match &channel {
            Some(channel) => channel.channel_id(),
            None => ChannelId::LOOPBACK,
        };

        let tier;
        let added = {
            let mut guard = self.data.lock().unwrap();
            tier = guard.rep_tiers.tier(&vote.voter);
            guard
                .queue
                .push((tier, channel_id), (vote, source, channel, filter))
        };

        if added {
            self.stats.inc(StatType::VoteProcessor, DetailType::Process);
            self.stats.inc(StatType::VoteProcessorTier, tier.into());
            self.condition.notify_one();
        } else {
            self.stats
                .inc(StatType::VoteProcessor, DetailType::Overfill);
            self.stats.inc(StatType::VoteProcessorOverfill, tier.into());
        }

        added
    }

    pub(crate) fn wait_for_votes(
        &self,
        max_batch_size: usize,
    ) -> VecDeque<(
        (RepTier, ChannelId),
        (
            Arc<Vote>,
            VoteSource,
            Option<Arc<Channel>>,
            Option<BlockHash>,
        ),
    )> {
        let mut guard = self.data.lock().unwrap();
        loop {
            if guard.stopped {
                return VecDeque::new();
            }

            if !guard.queue.is_empty() {
                return guard.queue.next_batch(max_batch_size);
            } else {
                guard = self.condition.wait(guard).unwrap();
            }
        }
    }

    pub fn clear(&self) {
        {
            let mut guard = self.data.lock().unwrap();
            guard.queue.clear();
        }
        self.condition.notify_all();
    }

    pub fn stop(&self) {
        {
            let mut guard = self.data.lock().unwrap();
            guard.stopped = true;
        }
        self.condition.notify_all();
    }

    pub fn stopped(&self) -> bool {
        self.data.lock().unwrap().stopped
    }

    pub fn info(&self) -> FairQueueInfo<RepTier> {
        self.data
            .lock()
            .unwrap()
            .queue
            .compacted_info(|(tier, _)| *tier)
    }
}

impl StatsSource for VoteProcessorQueue {
    fn collect_stats(&self, result: &mut StatsCollection) {
        result.insert("vote_processor_queue", "len_total", self.len() as u64);
    }
}

impl ContainerInfoProvider for VoteProcessorQueue {
    fn container_info(&self) -> ContainerInfo {
        let guard = self.data.lock().unwrap();
        ContainerInfo::builder()
            .leaf(
                "votes",
                guard.queue.len(),
                size_of::<(Arc<Vote>, VoteSource)>(),
            )
            .node("queue", guard.queue.container_info())
            .finish()
    }
}

impl RepTiersConsumer for VoteProcessorQueue {
    fn update_rep_tiers(&self, new_tiers: RepTiers) {
        self.data.lock().unwrap().rep_tiers = new_tiers;
    }
}

pub struct VoteProcessorQueueCleanup(Arc<VoteProcessorQueue>);

impl VoteProcessorQueueCleanup {
    pub fn new(queue: Arc<VoteProcessorQueue>) -> Self {
        Self(queue)
    }
}

impl DeadChannelCleanupStep for VoteProcessorQueueCleanup {
    fn clean_up_dead_channels(&self, dead_channel_ids: &[ChannelId]) {
        let mut guard = self.0.data.lock().unwrap();
        for channel_id in dead_channel_ids {
            for tier in RepTier::iter() {
                guard.queue.remove(&(tier, *channel_id));
            }
        }
    }
}

struct VoteProcessorQueueData {
    stopped: bool,
    queue: FairQueue<
        (RepTier, ChannelId),
        (
            Arc<Vote>,
            VoteSource,
            Option<Arc<Channel>>,
            Option<BlockHash>, //filter
        ),
    >,
    rep_tiers: RepTiers,
}
