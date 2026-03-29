use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use tracing::debug;

use rsnano_network::Channel;
use rsnano_types::{BlockHash, Vote, VoteError, VoteSource};
use rsnano_utils::{
    stats::{DetailType, StatType, Stats},
    sync::backpressure_channel::Sender,
};

use super::{AecFact, FilteredVote, ReceivedVote, VoteApplier, VoteProcessorQueue};

type QueuedVote = (
    Arc<Vote>,
    VoteSource,
    Option<Arc<Channel>>,
    Option<BlockHash>,
);

#[derive(Clone, Debug, PartialEq)]
pub struct VoteProcessorConfig {
    pub max_pr_queue: usize,
    pub max_non_pr_queue: usize,
    pub pr_priority: usize,
    pub threads: usize,
    pub batch_size: usize,
    pub max_triggered: usize,
}

impl VoteProcessorConfig {
    pub fn new(parallelism: usize) -> Self {
        Self {
            max_pr_queue: 256,
            max_non_pr_queue: 32,
            pr_priority: 3,
            threads: (parallelism / 2).clamp(1, 4),
            batch_size: 1024,
            max_triggered: 16384,
        }
    }
}

pub type VoteProcessedCallback2 =
    Box<dyn Fn(&Arc<Vote>, Option<&Arc<Channel>>, VoteSource, VoteError) + Send + Sync>;

pub struct VoteProcessor {
    threads: Mutex<Vec<JoinHandle<()>>>,
    queue: Arc<VoteProcessorQueue>,
    execution_queue: VoteExecutionQueue,
    vote_applier: VoteApplier,
    stats: Arc<Stats>,
    pub total_processed: AtomicU64,
    cool_down: AtomicBool,
}

impl VoteProcessor {
    pub(crate) fn new(
        queue: Arc<VoteProcessorQueue>,
        vote_applier: VoteApplier,
        stats: Arc<Stats>,
    ) -> Self {
        Self {
            queue,
            execution_queue: VoteExecutionQueue::default(),
            vote_applier,
            stats,
            threads: Mutex::new(Vec::new()),
            total_processed: AtomicU64::new(0),
            cool_down: AtomicBool::new(false),
        }
    }

    pub fn add_observer(&self, sink: Sender<AecFact>) {
        self.vote_applier.add_event_sink(sink);
    }

    pub fn cool_down(&self) {
        self.cool_down.store(true, Ordering::Relaxed);
    }

    pub fn recovered(&self) {
        self.cool_down.store(false, Ordering::Relaxed);
    }

    pub fn stop(&self) {
        self.vote_applier.stop();
        self.queue.stop();

        let mut handles = Vec::new();
        {
            let mut guard = self.threads.lock().unwrap();
            std::mem::swap(&mut handles, &mut guard);
        }
        for handle in handles {
            handle.join().unwrap()
        }
    }

    pub fn run(&self) {
        loop {
            if self.cool_down.load(Ordering::Relaxed) {
                if self.queue.stopped() {
                    return;
                }

                std::thread::sleep(Duration::from_millis(25));
                continue;
            }

            let Some(vote) = self.wait_for_next_vote() else {
                break; //stopped
            };

            self.total_processed.fetch_add(1, Ordering::SeqCst);
            let _ = self.process_queued_vote(vote);
        }
    }

    pub fn vote_blocking(&self, vote: &FilteredVote) -> Result<(), VoteError> {
        let mut result = Err(VoteError::Invalid);
        if vote.validate().is_ok() {
            let vote_results = self.vote_applier.vote(vote);
            result = aggregate_vote_results(&vote_results);
        }

        result
    }

    fn process_queued_vote(&self, vote: QueuedVote) -> Result<(), VoteError> {
        let (vote, source, channel, filter) = vote;
        let filter = filter.unwrap_or_default();
        let received_vote = ReceivedVote::new(vote, source, channel);
        let filtered_vote = FilteredVote::new(received_vote, filter);

        self.vote_blocking(&filtered_vote)
    }

    fn wait_for_next_vote(&self) -> Option<QueuedVote> {
        let start = Instant::now();
        let batch = self.execution_queue.wait_for_vote(
            || {
                self.stats.inc(StatType::VoteProcessor, DetailType::Loop);
                self.queue.wait_for_votes(self.queue.config.batch_size)
            },
            self.queue.config.batch_size,
        )?;

        let elapsed_millis = start.elapsed().as_millis();
        if batch.batch_size == self.queue.config.batch_size && elapsed_millis > 100 {
            debug!(
                "Dispatched {} votes in {} milliseconds",
                batch.batch_size, elapsed_millis
            );
        }

        Some(batch.vote)
    }
}

impl Drop for VoteProcessor {
    fn drop(&mut self) {
        // Thread must be stopped before destruction
        debug_assert!(self.threads.lock().unwrap().is_empty());
    }
}

pub trait VoteProcessorExt {
    fn start(&self);
}

impl VoteProcessorExt for Arc<VoteProcessor> {
    fn start(&self) {
        let mut threads = self.threads.lock().unwrap();
        debug_assert!(threads.is_empty());
        for _ in 0..self.queue.config.threads {
            let self_l = Arc::clone(self);
            threads.push(
                std::thread::Builder::new()
                    .name("Vote processing".to_string())
                    .spawn(Box::new(move || {
                        self_l.run();
                    }))
                    .unwrap(),
            )
        }
    }
}

// Aggregate results for individual hashes
pub fn aggregate_vote_results(
    results: &HashMap<BlockHash, Result<(), VoteError>>,
) -> Result<(), VoteError> {
    let mut ignored = false;
    let mut replay = false;
    let mut processed = false;
    let mut late = false;
    for res in results.values() {
        ignored |= matches!(res, Err(VoteError::Ignored));
        replay |= matches!(res, Err(VoteError::Replay));
        processed |= res.is_ok();
        late |= matches!(res, Err(VoteError::Late));
    }
    if ignored {
        Err(VoteError::Ignored)
    } else if replay {
        Err(VoteError::Replay)
    } else if processed {
        Ok(())
    } else if late {
        Err(VoteError::Late)
    } else {
        Err(VoteError::Indeterminate)
    }
}

struct VoteExecutionQueue {
    state: Mutex<VoteExecutionState>,
    condition: Condvar,
}

impl Default for VoteExecutionQueue {
    fn default() -> Self {
        Self {
            state: Mutex::new(VoteExecutionState::default()),
            condition: Condvar::new(),
        }
    }
}

impl VoteExecutionQueue {
    fn wait_for_vote(
        &self,
        fetch_batch: impl FnOnce()
            -> VecDeque<((super::RepTier, rsnano_network::ChannelId), QueuedVote)>,
        configured_batch_size: usize,
    ) -> Option<ClaimedVote> {
        let mut fetch_batch = Some(fetch_batch);

        loop {
            let should_fetch = {
                let mut state = self.state.lock().unwrap();
                if let Some(vote) = state.pending.pop_front() {
                    return Some(ClaimedVote {
                        vote,
                        batch_size: state.last_batch_size.max(1),
                    });
                }

                if state.stopped {
                    return None;
                }

                if state.fetch_in_progress {
                    let _guard = self.condition.wait(state).unwrap();
                    false
                } else {
                    state.fetch_in_progress = true;
                    true
                }
            };

            if !should_fetch {
                continue;
            }

            let batch = fetch_batch
                .take()
                .expect("batch fetch closure must only be consumed once")();
            let batch_size = batch.len().min(configured_batch_size);

            let mut state = self.state.lock().unwrap();
            state.fetch_in_progress = false;
            state.last_batch_size = batch_size;

            if batch.is_empty() {
                state.stopped = true;
            } else {
                state
                    .pending
                    .extend(batch.into_iter().map(|(_, vote)| vote));
            }
            self.condition.notify_all();
        }
    }
}

#[derive(Default)]
struct VoteExecutionState {
    pending: VecDeque<QueuedVote>,
    fetch_in_progress: bool,
    stopped: bool,
    last_batch_size: usize,
}

struct ClaimedVote {
    vote: QueuedVote,
    batch_size: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_types::{PrivateKey, Vote};
    use std::{
        sync::{Arc, mpsc},
        time::Duration,
    };

    #[test]
    fn two_votes_from_one_batch_can_begin_execution_in_parallel() {
        let execution_queue = Arc::new(VoteExecutionQueue::default());
        {
            let mut state = execution_queue.state.lock().unwrap();
            state.last_batch_size = 2;
            state.pending.push_back((
                Arc::new(Vote::new(
                    &PrivateKey::from(1),
                    Vote::TIMESTAMP_MIN,
                    0,
                    vec![BlockHash::from(1)],
                )),
                VoteSource::Live,
                None,
                None,
            ));
            state.pending.push_back((
                Arc::new(Vote::new(
                    &PrivateKey::from(2),
                    Vote::TIMESTAMP_MIN,
                    0,
                    vec![BlockHash::from(2)],
                )),
                VoteSource::Live,
                None,
                None,
            ));
        }

        let (started_tx, started_rx) = mpsc::channel();
        let mut handles = Vec::new();
        for _ in 0..2 {
            let execution_queue = execution_queue.clone();
            let started_tx = started_tx.clone();
            handles.push(std::thread::spawn(move || {
                let claimed = execution_queue
                    .wait_for_vote(
                        || unreachable!("pending votes should be claimed directly"),
                        2,
                    )
                    .unwrap();
                started_tx.send(claimed.batch_size).unwrap();
            }));
        }

        assert_eq!(started_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 2);
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 2);

        for handle in handles {
            handle.join().unwrap();
        }
    }
}
