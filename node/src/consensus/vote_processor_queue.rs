use std::{
    collections::{HashMap, VecDeque},
    mem::size_of,
    sync::{
        Arc, Condvar, Mutex, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
};

use strum::IntoEnumIterator;

use rsnano_network::{Channel, ChannelId, DeadChannelCleanupStep};
use rsnano_types::{BlockHash, Vote, VoteSource};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    fair_queue::{FairQueue, FairQueueInfo},
    stats::{DetailType, StatType, Stats},
};

use super::{RepTier, RepTiers, RepTiersConsumer, VoteProcessorConfig};

type QueueKey = (RepTier, ChannelId);
type QueuedVote = (
    Arc<Vote>,
    VoteSource,
    Option<Arc<Channel>>,
    Option<BlockHash>,
);

pub struct VoteProcessorQueue {
    state: Mutex<VoteProcessorQueueState>,
    condition: Condvar,
    rep_tiers: RwLock<RepTiers>,
    reservations: IngressReservations,
    ingress_tx: Sender<IngressRequest>,
    queued_len: AtomicUsize,
    stopped: AtomicBool,
    pub config: VoteProcessorConfig,
    stats: Arc<Stats>,
}

impl VoteProcessorQueue {
    pub fn new(config: VoteProcessorConfig, stats: Arc<Stats>) -> Self {
        let conf = config.clone();
        let (ingress_tx, ingress_rx) = mpsc::channel();

        Self {
            state: Mutex::new(VoteProcessorQueueState {
                ingress_rx,
                queue: FairQueue::new(
                    move |(tier, channel)| {
                        let max_size = match tier {
                            RepTier::Tier1 | RepTier::Tier2 | RepTier::Tier3 => conf.max_pr_queue,
                            RepTier::None => conf.max_non_pr_queue,
                        };
                        if *channel == ChannelId::LOOPBACK {
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
            rep_tiers: RwLock::new(RepTiers::default()),
            reservations: IngressReservations::new(),
            ingress_tx,
            queued_len: AtomicUsize::new(0),
            stopped: AtomicBool::new(false),
            config,
            stats,
        }
    }

    pub fn new_null() -> Self {
        Self::new(VoteProcessorConfig::new(1), Stats::default().into())
    }

    pub fn len(&self) -> usize {
        self.queued_len.load(Ordering::Acquire)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Queue vote for processing. @returns true if the vote was queued
    pub fn enqueue(
        &self,
        vote: Arc<Vote>,
        channel: Option<Arc<Channel>>,
        source: VoteSource,
        filter: Option<BlockHash>,
    ) -> bool {
        if self.stopped() {
            return false;
        }

        let channel_id = match &channel {
            Some(channel) => channel.channel_id(),
            None => ChannelId::LOOPBACK,
        };

        let tier = self.rep_tiers.read().unwrap().tier(&vote.voter);
        let key = (tier, channel_id);
        let max_size = self.max_size_for(&key);

        if !self.reservations.try_reserve(key, max_size) {
            self.stats
                .inc(StatType::VoteProcessor, DetailType::Overfill);
            self.stats.inc(StatType::VoteProcessorOverfill, tier.into());
            return false;
        }

        let send_result = self.ingress_tx.send(IngressRequest {
            key,
            vote,
            source,
            channel,
            filter,
        });

        if send_result.is_err() {
            self.reservations.release(key);
            return false;
        }

        self.queued_len.fetch_add(1, Ordering::AcqRel);
        self.stats.inc(StatType::VoteProcessor, DetailType::Process);
        self.stats.inc(StatType::VoteProcessorTier, tier.into());
        self.condition.notify_one();
        true
    }

    pub(crate) fn wait_for_votes(
        &self,
        max_batch_size: usize,
    ) -> VecDeque<(QueueKey, QueuedVote)> {
        let mut guard = self.state.lock().unwrap();
        loop {
            self.drain_ingress(&mut guard);

            if self.stopped() {
                return VecDeque::new();
            }

            if !guard.queue.is_empty() {
                let batch = guard.queue.next_batch(max_batch_size);
                for (key, _) in &batch {
                    self.release_ingress_slot(*key);
                }
                return batch;
            }

            guard = self.condition.wait(guard).unwrap();
        }
    }

    pub fn clear(&self) {
        let mut guard = self.state.lock().unwrap();
        self.drain_ingress(&mut guard);

        let queued_len = guard.queue.len();
        let queued = guard.queue.next_batch(queued_len);
        for (key, _) in queued {
            self.release_ingress_slot(key);
        }

        self.condition.notify_all();
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.clear();
        self.condition.notify_all();
    }

    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    pub fn info(&self) -> FairQueueInfo<RepTier> {
        self.state
            .lock()
            .unwrap()
            .queue
            .compacted_info(|(tier, _)| *tier)
    }

    fn max_size_for(&self, key: &QueueKey) -> usize {
        let (tier, channel_id) = key;
        let base = match tier {
            RepTier::Tier1 | RepTier::Tier2 | RepTier::Tier3 => self.config.max_pr_queue,
            RepTier::None => self.config.max_non_pr_queue,
        };
        if *channel_id == ChannelId::LOOPBACK {
            base * 10
        } else {
            base
        }
    }

    fn drain_ingress(&self, state: &mut VoteProcessorQueueState) {
        loop {
            match state.ingress_rx.try_recv() {
                Ok(request) => {
                    let added = state.queue.push(
                        request.key,
                        (request.vote, request.source, request.channel, request.filter),
                    );
                    debug_assert!(added, "reserved ingress slot must accept queued vote");
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    fn release_ingress_slot(&self, key: QueueKey) {
        self.reservations.release(key);
        self.queued_len.fetch_sub(1, Ordering::AcqRel);
    }
}

impl ContainerInfoProvider for VoteProcessorQueue {
    fn container_info(&self) -> ContainerInfo {
        let guard = self.state.lock().unwrap();
        let fair_queue_len = guard.queue.len();
        let total_len = self.len();
        ContainerInfo::builder()
            .leaf(
                "votes",
                total_len,
                size_of::<(Arc<Vote>, VoteSource)>(),
            )
            .leaf(
                "ingress",
                total_len.saturating_sub(fair_queue_len),
                size_of::<IngressRequest>(),
            )
            .node("queue", guard.queue.container_info())
            .finish()
    }
}

impl RepTiersConsumer for VoteProcessorQueue {
    fn update_rep_tiers(&self, new_tiers: RepTiers) {
        *self.rep_tiers.write().unwrap() = new_tiers;
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
        let mut guard = self.0.state.lock().unwrap();
        self.0.drain_ingress(&mut guard);

        for channel_id in dead_channel_ids {
            for tier in RepTier::iter() {
                let key = (tier, *channel_id);
                let removed = guard.queue.queue_len(&key);
                if removed > 0 {
                    guard.queue.remove(&key);
                    self.0.reservations.release_n(key, removed);
                    self.0.queued_len.fetch_sub(removed, Ordering::AcqRel);
                }
            }
        }
    }
}

struct VoteProcessorQueueState {
    ingress_rx: Receiver<IngressRequest>,
    queue: FairQueue<QueueKey, QueuedVote>,
}

struct IngressRequest {
    key: QueueKey,
    vote: Arc<Vote>,
    source: VoteSource,
    channel: Option<Arc<Channel>>,
    filter: Option<BlockHash>,
}

struct IngressReservations {
    shards: Box<[Mutex<HashMap<QueueKey, usize>>]>,
}

impl IngressReservations {
    const SHARD_COUNT: usize = 32;

    fn new() -> Self {
        let mut shards = Vec::with_capacity(Self::SHARD_COUNT);
        for _ in 0..Self::SHARD_COUNT {
            shards.push(Mutex::new(HashMap::new()));
        }
        Self {
            shards: shards.into_boxed_slice(),
        }
    }

    fn try_reserve(&self, key: QueueKey, max_size: usize) -> bool {
        let mut guard = self.shard(key).lock().unwrap();
        let len = guard.entry(key).or_default();
        if *len >= max_size {
            return false;
        }
        *len += 1;
        true
    }

    fn release(&self, key: QueueKey) {
        self.release_n(key, 1);
    }

    fn release_n(&self, key: QueueKey, count: usize) {
        let mut guard = self.shard(key).lock().unwrap();
        match guard.get_mut(&key) {
            Some(current) if *current > count => *current -= count,
            Some(_) => {
                guard.remove(&key);
            }
            None => debug_assert!(false, "ingress reservation missing for {key:?}"),
        }
    }

    fn shard(&self, key: QueueKey) -> &Mutex<HashMap<QueueKey, usize>> {
        let index = (tier_index(key.0) ^ channel_hash(key.1)) % self.shards.len();
        &self.shards[index]
    }
}

fn channel_hash(channel_id: ChannelId) -> usize {
    channel_id.as_usize()
}

fn tier_index(value: RepTier) -> usize {
    match value {
        RepTier::None => 0,
        RepTier::Tier1 => 1,
        RepTier::Tier2 => 2,
        RepTier::Tier3 => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_types::PrivateKey;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn enqueue_does_not_wait_for_dequeue_lock() {
        let queue = Arc::new(VoteProcessorQueue::new_null());
        let guard = queue.state.lock().unwrap();

        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let queue_l = queue.clone();
        let handle = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let added = queue_l.enqueue(test_vote(1), None, VoteSource::Live, None);
            done_tx.send(added).unwrap();
        });

        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap(), true);

        drop(guard);
        handle.join().unwrap();
    }

    fn test_vote(id: u64) -> Arc<Vote> {
        Arc::new(Vote::new(
            &PrivateKey::from(id),
            Vote::TIMESTAMP_MIN,
            0,
            vec![BlockHash::from(id)],
        ))
    }
}
