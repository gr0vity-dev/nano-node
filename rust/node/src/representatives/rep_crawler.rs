use super::{InsertResult, OnlineReps};
use crate::{
    config::NodeConfig,
    consensus::ActiveElections,
    stats::{DetailType, Direction, Sample, StatType, Stats},
    transport::{
        BufferDropPolicy, ChannelEnum, ChannelId, Network, PeerConnector, PeerConnectorExt,
        TrafficType, TransportType,
    },
    utils::{into_ipv6_socket_address, AsyncRuntime},
    NetworkParams,
};
use bounded_vec_deque::BoundedVecDeque;
use rsnano_core::{
    utils::{ContainerInfo, ContainerInfoComponent},
    BlockHash, Root, Vote,
};
use rsnano_ledger::Ledger;
use rsnano_messages::{ConfirmReq, Message};
use std::{
    collections::HashMap,
    mem::size_of,
    ops::DerefMut,
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tracing::{debug, error, info, warn};

/// Crawls the network for representatives. Queries are performed by requesting confirmation of a
/// random block and observing the corresponding vote.
pub struct RepCrawler {
    rep_crawler_impl: Mutex<RepCrawlerImpl>,
    online_reps: Arc<Mutex<OnlineReps>>,
    stats: Arc<Stats>,
    config: NodeConfig,
    network_params: NetworkParams,
    network: Arc<Network>,
    peer_connector: Arc<PeerConnector>,
    async_rt: Arc<AsyncRuntime>,
    condition: Condvar,
    ledger: Arc<Ledger>,
    active: Arc<ActiveElections>,
    thread: Mutex<Option<JoinHandle<()>>>,
    relative_time: Instant,
}

impl RepCrawler {
    const MAX_RESPONSES: usize = 1024 * 4;

    pub fn new(
        online_reps: Arc<Mutex<OnlineReps>>,
        stats: Arc<Stats>,
        query_timeout: Duration,
        config: NodeConfig,
        network_params: NetworkParams,
        network: Arc<Network>,
        async_rt: Arc<AsyncRuntime>,
        ledger: Arc<Ledger>,
        active: Arc<ActiveElections>,
        peer_connector: Arc<PeerConnector>,
        relative_time: Instant,
    ) -> Self {
        let is_dev_network = network_params.network.is_dev_network();
        Self {
            online_reps: Arc::clone(&online_reps),
            stats: Arc::clone(&stats),
            config,
            network_params,
            network: Arc::clone(&network),
            async_rt,
            condition: Condvar::new(),
            ledger,
            active,
            thread: Mutex::new(None),
            peer_connector,
            relative_time,
            rep_crawler_impl: Mutex::new(RepCrawlerImpl {
                is_dev_network,
                queries: OrderedQueries::new(),
                online_reps,
                stats,
                query_timeout,
                stopped: false,
                last_query: None,
                responses: BoundedVecDeque::new(Self::MAX_RESPONSES),
                network,
            }),
        }
    }

    pub fn stop(&self) {
        {
            let mut guard = self.rep_crawler_impl.lock().unwrap();
            guard.stopped = true;
        }
        self.condition.notify_all();
        if let Some(handle) = self.thread.lock().unwrap().take() {
            handle.join().unwrap();
        }
    }

    /// Called when a non-replay vote arrives that might be of interest to rep crawler.
    /// @return true, if the vote was of interest and was processed, this indicates that the rep is likely online and voting
    pub fn process(&self, vote: Arc<Vote>, channel: Arc<ChannelEnum>) -> bool {
        let mut guard = self.rep_crawler_impl.lock().unwrap();
        let mut processed = false;

        let query_timeout = guard.query_timeout;
        let x = guard.deref_mut();
        let queries = &mut x.queries;
        let responses = &mut x.responses;
        queries.modify_for_channel(channel.channel_id(), |query| {
            // TODO: This linear search could be slow, especially with large votes.
            let target_hash = query.hash;
            let found = vote.hashes.iter().any(|h| *h == target_hash);
            let done;

            if found {
                debug!(
                    "Processing response for block: {} from: {}",
                    target_hash,
                    channel.remote_addr()
                );
                self.stats
                    .inc_dir(StatType::RepCrawler, DetailType::Response, Direction::In);

                self.stats.sample(
                    Sample::RepResponseTime,
                    query.time.elapsed().as_millis() as i64,
                    (0, query_timeout.as_millis() as i64),
                );

                responses.push_back((Arc::clone(&channel), Arc::clone(&vote)));
                query.replies += 1;
                self.condition.notify_all();
                processed = true;
                done = true
            } else {
                done = false
            }

            done
        });

        processed
    }

    /// Attempt to determine if the peer manages one or more representative accounts
    pub fn query(&self, target_channels: Vec<Arc<ChannelEnum>>) {
        let Some(hash_root) = self.prepare_query_target() else {
            debug!("No block to query");
            self.stats.inc_dir(
                StatType::RepCrawler,
                DetailType::QueryTargetFailed,
                Direction::In,
            );
            return;
        };

        let mut guard = self.rep_crawler_impl.lock().unwrap();

        for channel in target_channels {
            guard.track_rep_request(
                hash_root,
                Arc::clone(&channel),
                self.relative_time.elapsed(),
            );
            debug!(
                "Sending query for block: {} to: {}",
                hash_root.0,
                channel.remote_addr()
            );
            self.stats
                .inc_dir(StatType::RepCrawler, DetailType::QuerySent, Direction::In);

            let req = Message::ConfirmReq(ConfirmReq::new(vec![hash_root]));

            let stats = Arc::clone(&self.stats);
            channel.send_obsolete(
                &req,
                Some(Box::new(move |ec, _len| {
                    if ec.is_err() {
                        stats.inc_dir(StatType::RepCrawler, DetailType::WriteError, Direction::Out);
                    }
                })),
                BufferDropPolicy::NoSocketDrop,
                TrafficType::Generic,
            )
        }
    }

    /// Attempt to determine if the peer manages one or more representative accounts
    pub fn query_channel(&self, target_channel: Arc<ChannelEnum>) {
        self.query(vec![target_channel]);
    }

    // Only for tests
    pub fn force_process(&self, vote: Arc<Vote>, channel: Arc<ChannelEnum>) {
        assert!(self.network_params.network.is_dev_network());
        let mut guard = self.rep_crawler_impl.lock().unwrap();
        guard.responses.push_back((channel, vote));
    }

    // Only for tests
    pub fn force_query(&self, hash: BlockHash, channel: Arc<ChannelEnum>) {
        assert!(self.network_params.network.is_dev_network());
        let mut guard = self.rep_crawler_impl.lock().unwrap();
        guard.queries.insert(QueryEntry {
            hash,
            channel,
            time: Instant::now(),
            replies: 0,
        })
    }

    fn run(&self) {
        let mut guard = self.rep_crawler_impl.lock().unwrap();
        while !guard.stopped {
            drop(guard);

            let current_total_weight;
            let sufficient_weight;
            {
                let reps = self.online_reps.lock().unwrap();
                current_total_weight = reps.peered_weight();
                sufficient_weight = current_total_weight > reps.quorum_delta();
            }

            // If online weight drops below minimum, reach out to preconfigured peers
            if !sufficient_weight {
                self.stats
                    .inc_dir(StatType::RepCrawler, DetailType::Keepalive, Direction::In);
                self.keepalive_preconfigured();
            }

            guard = self.rep_crawler_impl.lock().unwrap();
            let interval = self.query_interval(sufficient_weight);
            guard = self
                .condition
                .wait_timeout_while(guard, interval, |i| {
                    !i.stopped && !i.query_predicate(interval) && i.responses.is_empty()
                })
                .unwrap()
                .0;

            if guard.stopped {
                return;
            }

            self.stats
                .inc_dir(StatType::RepCrawler, DetailType::Loop, Direction::In);

            if !guard.responses.is_empty() {
                self.validate_and_process(guard);
                guard = self.rep_crawler_impl.lock().unwrap();
            }

            guard.cleanup();

            if guard.query_predicate(interval) {
                guard.last_query = Some(Instant::now());

                let targets =
                    guard.prepare_crawl_targets(sufficient_weight, self.relative_time.elapsed());
                drop(guard);
                self.query(targets);
                guard = self.rep_crawler_impl.lock().unwrap();
            }
        }
    }

    fn validate_and_process<'a>(&self, mut guard: MutexGuard<RepCrawlerImpl>) {
        let mut responses = BoundedVecDeque::new(Self::MAX_RESPONSES);
        std::mem::swap(&mut guard.responses, &mut responses);
        drop(guard);

        // normally the rep_crawler only tracks principal reps but it can be made to track
        // reps with less weight by setting rep_crawler_weight_minimum to a low value
        let minimum = std::cmp::min(
            self.online_reps.lock().unwrap().minimum_principal_weight(),
            self.config.rep_crawler_weight_minimum,
        );

        // TODO: Is it really faster to repeatedly lock/unlock the mutex for each response?
        for (channel, vote) in responses {
            if channel.get_type() == TransportType::Loopback {
                debug!(
                    "Ignoring vote from loopback channel: {}",
                    channel.channel_id()
                );
                continue;
            }

            let rep_weight = self.ledger.weight(&vote.voting_account);
            if rep_weight < minimum {
                debug!(
                    "Ignoring vote from account: {} with too little voting weight: {}",
                    vote.voting_account.encode_account(),
                    rep_weight.to_string_dec()
                );
                continue;
            }

            let endpoint = channel.remote_addr();
            let result = self.online_reps.lock().unwrap().vote_observed_directly(
                vote.voting_account,
                channel.channel_id(),
                self.relative_time.elapsed(),
            );

            match result {
                InsertResult::Inserted => {
                    info!(
                        "Found representative: {} at: {}",
                        vote.voting_account.encode_account(),
                        endpoint
                    );
                }
                InsertResult::ChannelChanged(previous) => {
                    warn!(
                        "Updated representative: {} at: {} (was at: {})",
                        vote.voting_account.encode_account(),
                        endpoint,
                        previous
                    )
                }
                InsertResult::Updated => {}
            }
        }
    }

    fn prepare_query_target(&self) -> Option<(BlockHash, Root)> {
        const MAX_ATTEMPTS: usize = 4;
        let tx = self.ledger.read_txn();
        let mut hash_root = None;

        // Randomly select a block from ledger to request votes for
        for _ in 0..MAX_ATTEMPTS {
            if hash_root.is_some() {
                break;
            }

            hash_root = self.ledger.hash_root_random(&tx);

            // Rebroadcasted votes for recently confirmed blocks might confuse the rep crawler
            if self
                .active
                .recently_confirmed
                .hash_exists(&hash_root.as_ref().unwrap().0)
            {
                hash_root = None;
            }
        }

        if hash_root.is_none() {
            return None;
        }

        // Don't send same block multiple times in tests
        if self.network_params.network.is_dev_network() {
            let guard = self.rep_crawler_impl.lock().unwrap();
            for _ in 0..MAX_ATTEMPTS {
                if guard.queries.count_by_block(&hash_root.as_ref().unwrap().0) == 0 {
                    break;
                }
                hash_root = self.ledger.hash_root_random(&tx);
            }
        }

        hash_root
    }

    fn query_interval(&self, sufficient_weight: bool) -> Duration {
        if sufficient_weight {
            self.network_params.network.rep_crawler_normal_interval
        } else {
            self.network_params.network.rep_crawler_warmup_interval
        }
    }

    pub fn keepalive_preconfigured(&self) {
        for peer in &self.config.preconfigured_peers {
            // can't use `network.port` here because preconfigured peers are referenced
            // just by their address, so we rely on them listening on the default port
            self.keepalive_or_connect(peer.clone(), self.network_params.network.default_node_port);
        }
    }

    pub fn keepalive_or_connect(&self, address: String, port: u16) {
        let peer_connector = self.peer_connector.clone();
        let network = self.network.clone();
        self.async_rt.tokio.spawn(async move {
            match tokio::net::lookup_host((address.as_str(), port)).await {
                Ok(addresses) => {
                    for address in addresses {
                        let endpoint = into_ipv6_socket_address(address);
                        match network.find_channel_by_peering_addr(&endpoint) {
                            Some(channel) => {
                                let keepalive = network.create_keepalive_message();
                                channel.send_obsolete(
                                    &keepalive,
                                    None,
                                    BufferDropPolicy::Limiter,
                                    TrafficType::Generic,
                                )
                            }
                            None => {
                                peer_connector.connect_to(endpoint);
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(
                        "Error resolving address for keepalive: {}:{} ({})",
                        address, port, e
                    )
                }
            }
        });
    }

    pub fn collect_container_info(&self, name: impl Into<String>) -> ContainerInfoComponent {
        let guard = self.rep_crawler_impl.lock().unwrap();
        ContainerInfoComponent::Composite(
            name.into(),
            vec![
                ContainerInfoComponent::Leaf(ContainerInfo {
                    name: "queries".to_string(),
                    count: guard.queries.len(),
                    sizeof_element: OrderedQueries::ELEMENT_SIZE,
                }),
                ContainerInfoComponent::Leaf(ContainerInfo {
                    name: "responses".to_string(),
                    count: guard.responses.len(),
                    sizeof_element: size_of::<Arc<Vote>>() * 2,
                }),
            ],
        )
    }
}

impl Drop for RepCrawler {
    fn drop(&mut self) {
        // Thread must be stopped before destruction
        debug_assert!(self.thread.lock().unwrap().is_none())
    }
}

struct RepCrawlerImpl {
    queries: OrderedQueries,
    online_reps: Arc<Mutex<OnlineReps>>,
    stats: Arc<Stats>,
    network: Arc<Network>,
    query_timeout: Duration,
    stopped: bool,
    last_query: Option<Instant>,
    responses: BoundedVecDeque<(Arc<ChannelEnum>, Arc<Vote>)>,
    is_dev_network: bool,
}

impl RepCrawlerImpl {
    fn query_predicate(&self, query_interval: Duration) -> bool {
        match &self.last_query {
            Some(last) => last.elapsed() >= query_interval,
            None => true,
        }
    }

    fn prepare_crawl_targets(
        &self,
        sufficient_weight: bool,
        now: Duration,
    ) -> Vec<Arc<ChannelEnum>> {
        // TODO: Make these values configurable
        const CONSERVATIVE_COUNT: usize = 160;
        const AGGRESSIVE_COUNT: usize = 160;
        const CONSERVATIVE_MAX_ATTEMPTS: usize = 4;
        const AGGRESSIVE_MAX_ATTEMPTS: usize = 8;
        let rep_query_interval = if self.is_dev_network {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(60)
        };

        self.stats.inc_dir(
            StatType::RepCrawler,
            if sufficient_weight {
                DetailType::CrawlNormal
            } else {
                DetailType::CrawlAggressive
            },
            Direction::In,
        );

        // Crawl more aggressively if we lack sufficient total peer weight.
        let required_peer_count = if sufficient_weight {
            CONSERVATIVE_COUNT
        } else {
            AGGRESSIVE_COUNT
        };

        /* include channels with ephemeral remote ports */
        let mut random_peers = self.network.random_channels(required_peer_count, 0);

        random_peers.retain(|channel| {
            match self
                .online_reps
                .lock()
                .unwrap()
                .last_request_elapsed(channel.channel_id(), now)
            {
                Some(last_request_elapsed) => {
                    // Throttle queries to active reps
                    last_request_elapsed >= rep_query_interval
                }
                None => {
                    // Avoid querying the same peer multiple times when rep crawler is warmed up
                    let max_attemts = if sufficient_weight {
                        CONSERVATIVE_MAX_ATTEMPTS
                    } else {
                        AGGRESSIVE_MAX_ATTEMPTS
                    };
                    self.queries.count_by_channel(channel.channel_id()) < max_attemts
                }
            }
        });

        random_peers
    }

    fn track_rep_request(
        &mut self,
        hash_root: (BlockHash, Root),
        channel: Arc<ChannelEnum>,
        now: Duration,
    ) {
        self.queries.insert(QueryEntry {
            hash: hash_root.0,
            channel: Arc::clone(&channel),
            time: Instant::now(),
            replies: 0,
        });
        // Find and update the timestamp on all reps available on the endpoint (a single host may have multiple reps)
        self.online_reps
            .lock()
            .unwrap()
            .on_rep_request(channel.channel_id(), now);
    }

    fn cleanup(&mut self) {
        // Evict queries that haven't been responded to in a while
        self.queries.retain(|query| {
            if query.time.elapsed() < self.query_timeout {
                return true; // Retain
            }

            if query.replies == 0 {
                debug!(
                    "Aborting unresponsive query for block: {} from: {}",
                    query.hash,
                    query.channel.remote_addr()
                );
                self.stats.inc_dir(
                    StatType::RepCrawler,
                    DetailType::QueryTimeout,
                    Direction::In,
                );
            } else {
                debug!(
                    "Completion of query with: {} replies for block: {} from: {}",
                    query.replies,
                    query.hash,
                    query.channel.remote_addr()
                );
                self.stats.inc_dir(
                    StatType::RepCrawler,
                    DetailType::QueryCompletion,
                    Direction::In,
                );
            }

            false // Retain
        });
    }
}

struct QueryEntry {
    hash: BlockHash,
    channel: Arc<ChannelEnum>,
    time: Instant,
    /// number of replies to the query
    replies: usize,
}

struct OrderedQueries {
    entries: HashMap<usize, QueryEntry>,
    sequenced: Vec<usize>,
    by_channel: HashMap<ChannelId, Vec<usize>>,
    by_hash: HashMap<BlockHash, Vec<usize>>,
    next_id: usize,
}

impl OrderedQueries {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            sequenced: Vec::new(),
            by_channel: HashMap::new(),
            by_hash: HashMap::new(),
            next_id: 1,
        }
    }

    pub const ELEMENT_SIZE: usize =
        size_of::<QueryEntry>() + size_of::<BlockHash>() + size_of::<usize>() * 3;

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    fn insert(&mut self, entry: QueryEntry) {
        let entry_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.sequenced.push(entry_id);
        self.by_channel
            .entry(entry.channel.channel_id())
            .or_default()
            .push(entry_id);
        self.by_hash.entry(entry.hash).or_default().push(entry_id);
        self.entries.insert(entry_id, entry);
    }

    fn retain(&mut self, predicate: impl Fn(&QueryEntry) -> bool) {
        let mut to_delete = Vec::new();
        for (&id, entry) in &self.entries {
            if !predicate(entry) {
                to_delete.push(id);
            }
        }
        for id in to_delete {
            self.remove(id);
        }
    }

    fn remove(&mut self, entry_id: usize) {
        if let Some(entry) = self.entries.remove(&entry_id) {
            self.sequenced.retain(|id| *id != entry_id);
            if let Some(mut by_channel) = self.by_channel.remove(&entry.channel.channel_id()) {
                if by_channel.len() > 1 {
                    by_channel.retain(|i| *i != entry_id);
                    self.by_channel
                        .insert(entry.channel.channel_id(), by_channel);
                }
            }
            if let Some(mut by_hash) = self.by_hash.remove(&entry.hash) {
                if by_hash.len() > 1 {
                    by_hash.retain(|i| *i != entry_id);
                    self.by_hash.insert(entry.hash, by_hash);
                }
            }
        }
    }

    fn count_by_block(&self, hash: &BlockHash) -> usize {
        self.by_hash.get(hash).map(|i| i.len()).unwrap_or_default()
    }

    fn count_by_channel(&self, channel_id: ChannelId) -> usize {
        self.by_channel
            .get(&channel_id)
            .map(|i| i.len())
            .unwrap_or_default()
    }

    fn modify_for_channel(
        &mut self,
        channel_id: ChannelId,
        mut f: impl FnMut(&mut QueryEntry) -> bool,
    ) {
        if let Some(ids) = self.by_channel.get(&channel_id) {
            for id in ids {
                if let Some(entry) = self.entries.get_mut(id) {
                    let done = f(entry);
                    if done {
                        return;
                    }
                }
            }
        }
    }
}

pub trait RepCrawlerExt {
    fn start(&self);
}

impl RepCrawlerExt for Arc<RepCrawler> {
    fn start(&self) {
        debug_assert!(self.thread.lock().unwrap().is_none());
        let self_l = Arc::clone(self);
        *self.thread.lock().unwrap() = Some(
            std::thread::Builder::new()
                .name("Rep Crawler".to_string())
                .spawn(Box::new(move || {
                    self_l.run();
                }))
                .unwrap(),
        );
    }
}
