use std::{
    cmp::min,
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use crate::ledger_factory::default_ledger_store_factory;
use rsnano_ledger::{AnySet, ConfirmedSet, Ledger, OwningAnySet};
use rsnano_messages::{
    AccountInfoAckPayload, AccountInfoReqPayload, AscPullAck, AscPullAckType, AscPullReq,
    AscPullReqType, BlocksAckPayload, BlocksReqPayload, FrontiersReqPayload, HashType, Message,
};
use rsnano_network::{
    Channel, ChannelId, DeadChannelCleanupStep, TrafficType, token_bucket::TokenBucket,
};
use rsnano_nullable_clock::SteadyClock;
use rsnano_types::{Block, BlockHash, Frontier};
use rsnano_utils::{
    fair_queue::FairQueue,
    stats::{DetailType, Direction, StatType, Stats},
};

use crate::transport::MessageSender;

#[derive(Clone, Debug, PartialEq)]
pub struct BootstrapServerConfig {
    pub max_queue: usize,
    pub threads: usize,
    pub batch_size: usize,
    pub limiter: usize,
}

impl Default for BootstrapServerConfig {
    fn default() -> Self {
        Self {
            max_queue: 16,
            threads: 1,
            batch_size: 64,
            limiter: 500,
        }
    }
}

/**
 * Processes bootstrap requests (`asc_pull_req` messages) and replies with bootstrap responses (`asc_pull_ack`)
 */
pub struct BootstrapServer {
    config: BootstrapServerConfig,
    stats: Arc<Stats>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    pub(crate) server_impl: Arc<BootstrapResponderImpl>,
    running: AtomicBool,
}

impl BootstrapServer {
    /** Maximum number of blocks to send in a single response, cannot be higher than capacity of a single `asc_pull_ack` message */
    pub const MAX_BLOCKS: u8 = BlocksAckPayload::MAX_BLOCKS as u8;
    pub const MAX_FRONTIERS: usize = AscPullAck::MAX_FRONTIERS;

    pub(crate) fn new(
        config: BootstrapServerConfig,
        stats: Arc<Stats>,
        ledger: Arc<Ledger>,
        clock: Arc<SteadyClock>,
        message_sender: MessageSender,
    ) -> Self {
        let max_queue = config.max_queue;
        let server_impl = Arc::new(BootstrapResponderImpl {
            stats: Arc::clone(&stats),
            ledger,
            batch_size: config.batch_size,
            on_response: Arc::new(Mutex::new(None)),
            condition: Condvar::new(),
            stopped: AtomicBool::new(false),
            queue: Mutex::new(FairQueue::new(move |_| max_queue, |_| 1)),
            message_sender: Mutex::new(message_sender),
            limiter: Mutex::new(TokenBucket::with_burst_ratio(config.limiter, 3.0)),
            clock,
        });

        Self {
            config,
            stats: Arc::clone(&stats),
            threads: Mutex::new(Vec::new()),
            server_impl,
            running: AtomicBool::new(false),
        }
    }

    pub fn new_null() -> Self {
        BootstrapServer::new(
            BootstrapServerConfig::default(),
            Stats::default().into(),
            Ledger::new_null(default_ledger_store_factory()).into(),
            SteadyClock::new_null().into(),
            MessageSender::new_null(),
        )
    }

    pub fn start(&self) {
        debug_assert!(self.threads.lock().unwrap().is_empty());

        let mut threads = self.threads.lock().unwrap();
        for _ in 0..self.config.threads {
            let server_impl = Arc::clone(&self.server_impl);
            threads.push(
                std::thread::Builder::new()
                    .name("Bootstrap serv".to_string())
                    .spawn(move || {
                        server_impl.run();
                    })
                    .unwrap(),
            );
        }

        self.running.store(true, Ordering::Relaxed);
    }

    pub fn stop(&self) {
        self.server_impl.stopped.store(true, Ordering::SeqCst);
        self.server_impl.condition.notify_all();

        let mut threads = self.threads.lock().unwrap();
        for thread in threads.drain(..) {
            thread.join().unwrap();
        }

        self.running.store(false, Ordering::Relaxed);
    }

    pub fn set_response_callback(&self, cb: Box<dyn Fn(&AscPullAck, &Arc<Channel>) + Send + Sync>) {
        *self.server_impl.on_response.lock().unwrap() = Some(cb);
    }

    pub fn enqueue(&self, message: AscPullReq, channel: Arc<Channel>) -> bool {
        if !self.running.load(Ordering::Relaxed) {
            return false;
        }

        if !self.verify(&message) {
            self.stats
                .inc(StatType::BootstrapServer, DetailType::Invalid);
            return false;
        }

        // If channel is full our response will be dropped anyway, so filter that early
        if channel.should_drop(TrafficType::BootstrapServer) {
            self.stats.inc_dir(
                StatType::BootstrapServer,
                DetailType::ChannelFull,
                Direction::In,
            );
            return false;
        }

        let req_type = DetailType::from(&message.req_type);
        let added = {
            let mut guard = self.server_impl.queue.lock().unwrap();
            guard.push(channel.channel_id(), (message, channel.clone()))
        };

        if added {
            self.stats
                .inc(StatType::BootstrapServer, DetailType::Request);
            self.stats.inc(StatType::BootstrapServerRequest, req_type);

            self.server_impl.condition.notify_one();
        } else {
            self.stats
                .inc(StatType::BootstrapServer, DetailType::Overfill);
            self.stats.inc(StatType::BootstrapServerOverfill, req_type);
        }

        added
    }

    fn verify(&self, message: &AscPullReq) -> bool {
        match &message.req_type {
            AscPullReqType::Blocks(i) => i.count > 0 && i.count <= Self::MAX_BLOCKS,
            AscPullReqType::AccountInfo(i) => !i.target.is_zero(),
            AscPullReqType::Frontiers(i) => i.count > 0 && i.count as usize <= Self::MAX_FRONTIERS,
        }
    }
}

impl Drop for BootstrapServer {
    fn drop(&mut self) {
        debug_assert!(self.threads.lock().unwrap().is_empty());
    }
}

pub(crate) struct BootstrapResponderImpl {
    stats: Arc<Stats>,
    ledger: Arc<Ledger>,
    on_response: Arc<Mutex<Option<Box<dyn Fn(&AscPullAck, &Arc<Channel>) + Send + Sync>>>>,
    stopped: AtomicBool,
    condition: Condvar,
    queue: Mutex<FairQueue<ChannelId, (AscPullReq, Arc<Channel>)>>,
    batch_size: usize,
    message_sender: Mutex<MessageSender>,
    limiter: Mutex<TokenBucket>,
    clock: Arc<SteadyClock>,
}

impl BootstrapResponderImpl {
    fn run(&self) {
        let mut queue = self.queue.lock().unwrap();
        while !self.stopped.load(Ordering::SeqCst) {
            queue = self
                .condition
                .wait_while(queue, |q| {
                    q.is_empty() && !self.stopped.load(Ordering::SeqCst)
                })
                .unwrap();

            // Rate limit the processing
            while !self.stopped.load(Ordering::SeqCst)
                && !self
                    .limiter
                    .lock()
                    .unwrap()
                    .try_consume(self.batch_size, self.clock.now())
            {
                self.stats
                    .inc(StatType::BootstrapServer, DetailType::Cooldown);
                queue = self
                    .condition
                    .wait_timeout(queue, Duration::from_millis(100))
                    .unwrap()
                    .0;
            }

            if self.stopped.load(Ordering::SeqCst) {
                return;
            }

            if !queue.is_empty() {
                self.stats.inc(StatType::BootstrapServer, DetailType::Loop);
                queue = self.run_batch(queue);
            }
        }
    }

    fn run_batch<'a>(
        &'a self,
        mut queue: MutexGuard<'a, FairQueue<ChannelId, (AscPullReq, Arc<Channel>)>>,
    ) -> MutexGuard<'a, FairQueue<ChannelId, (AscPullReq, Arc<Channel>)>> {
        let batch = queue.next_batch(self.batch_size);
        drop(queue);

        let mut any = self.ledger.any();
        for (_, (request, channel)) in batch {
            if any.should_refresh() {
                any = self.ledger.any();
            }

            if !channel.should_drop(TrafficType::BootstrapServer) {
                let response = self.process(&any, request);
                self.respond(response, &channel);
            } else {
                self.stats.inc_dir(
                    StatType::BootstrapServer,
                    DetailType::ChannelFull,
                    Direction::Out,
                );
            }
        }

        self.queue.lock().unwrap()
    }

    fn process(&self, any: &OwningAnySet, message: AscPullReq) -> AscPullAck {
        match message.req_type {
            AscPullReqType::Blocks(blocks) => self.process_blocks(any, message.id, blocks),
            AscPullReqType::AccountInfo(account) => self.process_account(any, message.id, account),
            AscPullReqType::Frontiers(frontiers) => {
                self.process_frontiers(any, message.id, frontiers)
            }
        }
    }

    fn process_blocks(&self, any: &dyn AnySet, id: u64, request: BlocksReqPayload) -> AscPullAck {
        let count = min(request.count, BootstrapServer::MAX_BLOCKS);

        match request.start_type {
            HashType::Account => {
                if let Some(info) = any.get_account(&request.start.into()) {
                    // Start from open block if pulling by account
                    return self.prepare_response(any, id, info.open_block, count);
                }
            }
            HashType::Block => {
                if any.block_exists(&request.start.into()) {
                    return self.prepare_response(any, id, request.start.into(), count);
                }
            }
        }

        // Neither block nor account found, send empty response to indicate that
        self.prepare_empty_blocks_response(id)
    }

    /*
     * Account info request
     */

    fn process_account(
        &self,
        any: &dyn AnySet,
        id: u64,
        request: AccountInfoReqPayload,
    ) -> AscPullAck {
        let target = match request.target_type {
            HashType::Account => request.target.into(),
            HashType::Block => {
                // Try to lookup account assuming target is block hash
                any.block_account(&request.target.into())
                    .unwrap_or_default()
            }
        };

        let mut response_payload = AccountInfoAckPayload {
            account: target,
            ..Default::default()
        };

        if let Some(account_info) = any.get_account(&target) {
            response_payload.account_open = account_info.open_block;
            response_payload.account_head = account_info.head;
            response_payload.account_block_count = account_info.block_count;

            if let Some(conf_info) = any.confirmed().get_conf_info(&target) {
                response_payload.account_conf_frontier = conf_info.frontier;
                response_payload.account_conf_height = conf_info.height;
            }
        }
        // If account is missing the response payload will contain all 0 fields, except for the target
        //
        AscPullAck {
            id,
            pull_type: AscPullAckType::AccountInfo(response_payload),
        }
    }

    /*
     * Frontiers request
     */
    fn process_frontiers(
        &self,
        any: &OwningAnySet,
        id: u64,
        request: FrontiersReqPayload,
    ) -> AscPullAck {
        let frontiers = any
            .accounts_range(request.start..)
            .map(|(account, info)| Frontier::new(account, info.head))
            .take(request.count as usize)
            .collect();

        AscPullAck {
            id,
            pull_type: AscPullAckType::Frontiers(frontiers),
        }
    }

    fn prepare_response(
        &self,
        any: &dyn AnySet,
        id: u64,
        start_block: BlockHash,
        count: u8,
    ) -> AscPullAck {
        let blocks = self.prepare_blocks(any, start_block, count as usize);
        let response_payload = BlocksAckPayload::new(blocks);

        AscPullAck {
            id,
            pull_type: AscPullAckType::Blocks(response_payload),
        }
    }

    fn prepare_empty_blocks_response(&self, id: u64) -> AscPullAck {
        AscPullAck {
            id,
            pull_type: AscPullAckType::Blocks(BlocksAckPayload::new(VecDeque::new())),
        }
    }

    fn prepare_blocks(
        &self,
        any: &dyn AnySet,
        start_block: BlockHash,
        count: usize,
    ) -> VecDeque<Block> {
        let mut result = VecDeque::new();
        if !start_block.is_zero() {
            let mut current = any.get_block(&start_block);
            while let Some(c) = current.take() {
                let successor = any.block_successor(&c.hash()).unwrap_or_default();
                result.push_back(c.into());

                if result.len() == count {
                    break;
                }
                current = any.get_block(&successor);
            }
        }
        result
    }

    fn respond(&self, response: AscPullAck, channel: &Arc<Channel>) {
        self.stats.inc_dir(
            StatType::BootstrapServer,
            DetailType::Response,
            Direction::Out,
        );
        self.stats.inc(
            StatType::BootstrapServerResponse,
            DetailType::from(&response.pull_type),
        );

        // Increase relevant stats depending on payload type
        match &response.pull_type {
            AscPullAckType::Blocks(blocks) => {
                self.stats.add_dir(
                    StatType::BootstrapServer,
                    DetailType::Blocks,
                    Direction::Out,
                    blocks.blocks().len() as u64,
                );
            }
            AscPullAckType::AccountInfo(_) => {
                self.stats.inc_dir(
                    StatType::BootstrapServer,
                    DetailType::AccountInfo,
                    Direction::Out,
                );
            }
            AscPullAckType::Frontiers(frontiers) => {
                self.stats.add_dir(
                    StatType::BootstrapServer,
                    DetailType::Frontiers,
                    Direction::Out,
                    frontiers.len() as u64,
                );
            }
        }

        {
            let callback = self.on_response.lock().unwrap();
            if let Some(cb) = &*callback {
                (cb)(&response, channel);
            }
        }

        let msg = Message::AscPullAck(response);
        self.message_sender
            .lock()
            .unwrap()
            .try_send(channel, &msg, TrafficType::BootstrapServer);
    }
}

pub(crate) struct BootstrapResponderCleanup(Arc<BootstrapResponderImpl>);

impl BootstrapResponderCleanup {
    pub fn new(server: Arc<BootstrapResponderImpl>) -> Self {
        Self(server)
    }
}

impl DeadChannelCleanupStep for BootstrapResponderCleanup {
    fn clean_up_dead_channels(&self, dead_channel_ids: &[ChannelId]) {
        let mut queue = self.0.queue.lock().unwrap();
        for channel_id in dead_channel_ids {
            queue.remove(channel_id);
        }
    }
}
