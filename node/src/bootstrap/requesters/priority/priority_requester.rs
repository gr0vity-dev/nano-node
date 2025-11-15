use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use rsnano_ledger::Ledger;
use rsnano_network::Channel;
use rsnano_utils::stats::{StatsCollection, StatsSource};

use super::{
    pull_count_decider::PullCountDecider, pull_type_decider::PullTypeDecider,
    query_factory::QueryFactory,
};
use crate::{
    block_processing::{BlockProcessorQueue, BlockSource},
    bootstrap::{
        AscPullQuerySpec, BootstrapConfig, BootstrapPromise, PollResult, PromiseContext,
        requesters::channel_waiter::{ChannelWaiter, ChannelWaiterStats},
    },
};

pub(crate) struct PriorityRequester {
    state: PriorityState,
    block_processor_queue: Arc<BlockProcessorQueue>,
    channel_waiter: ChannelWaiter,
    pub block_processor_threshold: usize,
    query_factory: QueryFactory,
    stats: Arc<PriorityRequesterStats>,
}

impl PriorityRequester {
    pub(crate) fn new(
        block_processor_queue: Arc<BlockProcessorQueue>,
        channel_waiter: ChannelWaiter,
        ledger: Arc<Ledger>,
        config: &BootstrapConfig,
    ) -> Self {
        let pull_type_decider = PullTypeDecider::new(config.optimistic_request_percentage);
        let pull_count_decider = PullCountDecider::new(config.max_pull_count);
        let query_factory = QueryFactory::new(ledger, pull_type_decider, pull_count_decider);
        let stats = Arc::new(PriorityRequesterStats {
            channel_waiter: channel_waiter.stats(),
            ..Default::default()
        });

        Self {
            state: PriorityState::Initial,
            block_processor_queue,
            stats,
            channel_waiter,
            query_factory,
            block_processor_threshold: 1000,
        }
    }

    pub fn stats(&self) -> Arc<PriorityRequesterStats> {
        self.stats.clone()
    }

    fn block_processor_free(&self) -> bool {
        self.block_processor_queue.queue_len(BlockSource::Bootstrap)
            < self.block_processor_threshold
    }
}

enum PriorityState {
    Initial,
    WaitBlockProcessor,
    WaitChannel,
    WaitPriority(Arc<Channel>),
}

impl BootstrapPromise<AscPullQuerySpec> for PriorityRequester {
    fn poll(&mut self, context: &mut PromiseContext) -> PollResult<AscPullQuerySpec> {
        match self.state {
            PriorityState::Initial => {
                self.stats.loop_count.fetch_add(1, Ordering::Relaxed);
                self.state = PriorityState::WaitBlockProcessor;
                PollResult::Progress
            }
            PriorityState::WaitBlockProcessor => {
                if self.block_processor_free() {
                    self.state = PriorityState::WaitChannel;
                    PollResult::Progress
                } else {
                    self.stats
                        .wait_block_processor
                        .fetch_add(1, Ordering::Relaxed);
                    PollResult::Wait
                }
            }
            PriorityState::WaitChannel => match self.channel_waiter.poll(context) {
                PollResult::Progress => PollResult::Progress,
                PollResult::Wait => PollResult::Wait,
                PollResult::Finished(channel) => {
                    self.state = PriorityState::WaitPriority(channel);
                    PollResult::Progress
                }
            },
            PriorityState::WaitPriority(ref channel) => {
                match self
                    .query_factory
                    .next_priority_query(context, channel.clone())
                {
                    Some(query) => {
                        self.stats.next.fetch_add(1, Ordering::Relaxed);
                        self.state = PriorityState::Initial;
                        PollResult::Finished(query)
                    }
                    _ => {
                        self.stats.wait_priority.fetch_add(1, Ordering::Relaxed);
                        PollResult::Wait
                    }
                }
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct PriorityRequesterStats {
    pub loop_count: AtomicU64,
    pub wait_block_processor: AtomicU64,
    pub wait_priority: AtomicU64,
    pub channel_waiter: Arc<ChannelWaiterStats>,
    pub next: AtomicU64,
}

impl StatsSource for PriorityRequesterStats {
    fn collect_stats(&self, result: &mut StatsCollection) {
        const STAT_NAME: &'static str = "boot_requester_prio";

        result.insert(STAT_NAME, "loop", self.loop_count.load(Ordering::Relaxed));
        result.insert(
            STAT_NAME,
            "wait_block_processor",
            self.wait_block_processor.load(Ordering::Relaxed),
        );
        result.insert(
            STAT_NAME,
            "wait_priority",
            self.wait_priority.load(Ordering::Relaxed),
        );

        result.insert(
            "bootstrap_next",
            "next_priority",
            self.next.load(Ordering::Relaxed),
        );

        self.channel_waiter.collect_stats(STAT_NAME, result);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, RwLock};

    use rsnano_ledger::Ledger;
    use rsnano_network::{Network, token_bucket::TokenBucket};
    use rsnano_types::Account;

    use super::PriorityRequester;
    use crate::{
        block_processing::BlockProcessorQueue,
        bootstrap::{
            BootstrapConfig, PollResult, progress_state,
            requesters::{
                channel_waiter::ChannelWaiter, priority::priority_requester::PriorityState,
            },
            state::BootstrapLogic,
        },
    };

    #[test]
    fn happy_path() {
        let mut state = BootstrapLogic::default();
        let account = Account::from(42);
        state.candidate_accounts.priority_up(&account);

        let (mut requester, network) = create_requester();
        network.write().unwrap().add_test_channel();
        let PollResult::Finished(result) = progress_state(&mut requester, &mut state) else {
            panic!("Finished expected");
        };

        assert_eq!(result.account, account);
    }

    #[test]
    fn wait_block_processor() {
        let mut state = BootstrapLogic::default();

        let (mut requester, _) = create_requester();
        requester.block_processor_threshold = 0;

        let result = progress_state(&mut requester, &mut state);

        assert!(matches!(result, PollResult::Wait));
        assert!(matches!(requester.state, PriorityState::WaitBlockProcessor));
    }

    #[test]
    fn wait_channel() {
        let mut state = BootstrapLogic::default();
        let (mut requester, _) = create_requester();

        let result = progress_state(&mut requester, &mut state);

        assert!(matches!(result, PollResult::Wait));
        assert!(matches!(requester.state, PriorityState::WaitChannel));
    }

    #[test]
    fn wait_priority() {
        let mut state = BootstrapLogic::default();
        let (mut requester, network) = create_requester();
        network.write().unwrap().add_test_channel();

        let result = progress_state(&mut requester, &mut state);

        assert!(matches!(result, PollResult::Wait));
        assert!(matches!(requester.state, PriorityState::WaitPriority(_)));
    }

    fn create_requester() -> (PriorityRequester, Arc<RwLock<Network>>) {
        let block_processor_queue = Arc::new(BlockProcessorQueue::default());
        let network = Arc::new(RwLock::new(Network::new_test_instance()));
        let rate_limiter = Arc::new(Mutex::new(TokenBucket::new(1024)));
        let channel_waiter = ChannelWaiter::new(network.clone(), rate_limiter, 1024);
        let ledger = Arc::new(Ledger::new_null(
            rsnano_store_lmdb::null_ledger_store_factory(),
        ));
        let config = BootstrapConfig::default();

        let requester =
            PriorityRequester::new(block_processor_queue, channel_waiter, ledger, &config);

        (requester, network)
    }
}
