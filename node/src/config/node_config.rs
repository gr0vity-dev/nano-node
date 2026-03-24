use std::{cmp::max, net::Ipv6Addr, time::Duration};

use rsnano_network::NetworkConfig;
use rsnano_nullable_http_client::Url;
use rsnano_store_lmdb::LmdbConfig;
use rsnano_types::{
    Account, Amount, Peer, PublicKey,
    currency_constants::{
        PRECONFIGURED_PEERS_BETA, PRECONFIGURED_PEERS_LIVE, PRECONFIGURED_PEERS_TEST,
    },
};
use rsnano_wallet::default_preconfigured_representatives_for_live;
use rsnano_work::OpenClConfig;

use super::{DEV_NETWORK_PARAMS, NetworkParams, NetworkType, websocket_config::WebsocketConfig};
use crate::{
    block_processing::{
        BacklogScanConfig, LocalBlockBroadcasterConfig, ProcessQueueConfig,
        bounded_backlog::BoundedBacklogConfig,
    },
    bootstrap::{BootstrapConfig, BootstrapServerConfig},
    cementation::ConfirmingSetConfig,
    consensus::{
        ActiveElectionsConfig, BootstrapStaleElections, ForkCache, RebroadcastHistoryConfig,
        RequestAggregatorConfig, VoteCacheConfig, VoteProcessorConfig, VoteRebroadcastQueue,
        election_schedulers::{
            HintedSchedulerConfig, OptimisticSchedulerConfig, priority::PriorityBucketConfig,
        },
    },
    transport::MessageProcessorConfig,
};

#[derive(Clone, Debug, PartialEq)]
pub struct NodeConfig {
    pub default_peering_port: u16,
    pub enable_opencl: bool,
    pub enable_voting: bool,
    pub enable_vote_processor: bool,
    pub enable_priority_scheduler: bool,
    pub enable_optimistic_scheduler: bool,
    pub enable_hinted_scheduler: bool,
    pub enable_monitor: bool,
    pub enable_bounded_backlog: bool,
    pub enable_vote_rebroadcast: bool,
    pub enable_bootstrap_responder: bool,
    pub optimistic_scheduler: OptimisticSchedulerConfig,
    pub hinted_scheduler: HintedSchedulerConfig,
    pub priority_bucket: PriorityBucketConfig,
    pub bootstrap_fraction_numerator: u32,
    pub receive_minimum: Amount,
    pub online_weight_minimum: Amount,
    /// The minimum vote weight that a representative must have for its vote to be counted.
    /// All representatives above this weight will be kept in memory!
    pub representative_vote_weight_minimum: Amount,
    pub password_fanout: u32,
    pub io_threads: usize,
    pub network_threads: u32,
    pub work_threads: u32,
    pub opencl: OpenClConfig,
    pub background_threads: u32,
    pub signature_checker_threads: u32,
    pub bootstrap_initiator_threads: u32,
    pub bootstrap_serving_threads: u32,
    pub allow_local_peers: bool,
    pub vote_minimum: Amount,
    pub vote_generator_delay: Duration,
    pub unchecked_cutoff_time_s: i64,
    pub pow_sleep_interval_ns: i64,
    pub external_address: String,
    pub external_port: u16,
    pub use_memory_pools: bool,
    pub bootstrap: BootstrapConfig,
    pub bootstrap_server: BootstrapServerConfig,
    pub confirming_set_batch_time: Duration,
    pub max_work_generate_multiplier: f64,
    pub max_queued_requests: u32,
    pub request_aggregator_threads: u32,
    pub max_unchecked_blocks: u32,
    pub rep_crawler_weight_minimum: Amount,
    pub work_peers: Vec<Peer>,
    pub secondary_work_peers: Vec<Peer>,
    pub preconfigured_peers: Vec<Peer>,
    pub preconfigured_representatives: Vec<PublicKey>,
    pub callback_address: String,
    pub callback_port: u16,
    pub callback_target: String,
    pub websocket_config: WebsocketConfig,
    pub lmdb_config: LmdbConfig,
    pub vote_cache: VoteCacheConfig,
    pub rep_crawler_query_timeout: Duration,
    pub block_processor: ProcessQueueConfig,
    pub block_processor_threads: usize,
    pub active_elections: ActiveElectionsConfig,
    pub vote_processor: VoteProcessorConfig,
    pub tcp: TcpConfig,
    pub request_aggregator: RequestAggregatorConfig,
    pub message_processor: MessageProcessorConfig,
    pub local_block_broadcaster: LocalBlockBroadcasterConfig,
    pub confirming_set: ConfirmingSetConfig,
    pub monitor: MonitorConfig,
    pub backlog_scan: BacklogScanConfig,
    pub bounded_backlog: BoundedBacklogConfig,
    pub network_duplicate_filter_size: usize,
    pub network_duplicate_filter_cutoff: u64,
    pub network: NetworkConfig,

    /// Maximum confirmation history size
    pub confirmation_history_size: usize,
    pub fork_cache_max_size: usize,
    pub fork_cache_max_forks_per_root: usize,
    pub bootstrap_stale_threshold: Duration,
    pub vote_rebroadcaster_max_queue: usize,
    pub rebroadcast_history: RebroadcastHistoryConfig,
    pub cps_limit: u32,
}

impl NodeConfig {
    pub fn default_for(network: NetworkType, parallelism: usize) -> Self {
        let net_params = NetworkParams::new(network);
        Self::new(
            Some(net_params.network.default_node_port),
            &net_params,
            parallelism,
        )
    }

    pub fn new(
        peering_port: Option<u16>,
        network_params: &NetworkParams,
        parallelism: usize,
    ) -> Self {
        if peering_port == Some(0) {
            // comment for posterity:
            // - we used to consider ports being 0 a sentinel that meant to use a default port for that specific purpose
            // - the actual default value was determined based on the active network (e.g. dev network peering port = 44000)
            // - now, the 0 value means something different instead: user wants to let the OS pick a random port
            // - for the specific case of the peering port, after it gets picked, it can be retrieved by client code via
            //   node.network.endpoint ().port ()
            // - the config value does not get back-propagated because it represents the choice of the user, and that was 0
        }

        let mut enable_voting = false;
        let mut preconfigured_peers = Vec::new();
        let mut preconfigured_representatives = Vec::new();
        let default_port = network_params.network.default_node_port;
        let network = network_params.network.current_network;
        match network {
            NetworkType::NanoDevNetwork => {
                enable_voting = true;
                preconfigured_representatives.push(network_params.ledger.genesis_account.into());
            }
            NetworkType::NanoBetaNetwork => {
                preconfigured_peers.extend(
                    PRECONFIGURED_PEERS_BETA
                        .iter()
                        .map(|p| Peer::new(*p, default_port)),
                );
                preconfigured_representatives.push(
                    Account::parse(
                        "nano_1defau1t9off1ine9rep99999999999999999999999999999999wgmuzxxy",
                    )
                    .unwrap()
                    .into(),
                );
            }
            NetworkType::NanoLiveNetwork => {
                preconfigured_peers.extend(
                    PRECONFIGURED_PEERS_LIVE
                        .iter()
                        .map(|p| Peer::new(*p, default_port)),
                );

                preconfigured_representatives = default_preconfigured_representatives_for_live();
            }
            NetworkType::NanoTestNetwork => {
                preconfigured_peers.extend(
                    PRECONFIGURED_PEERS_TEST
                        .iter()
                        .map(|p| Peer::new(*p, default_port)),
                );
                preconfigured_representatives.push(network_params.ledger.genesis_account.into());
            }
            NetworkType::Invalid => panic!("invalid network"),
        }

        let block_processor_cfg = ProcessQueueConfig::default();

        let allow_local_peers =
            !(network_params.network.is_live_network() || network_params.network.is_test_network());

        Self {
            enable_opencl: false,
            enable_voting,
            enable_vote_processor: true,
            enable_priority_scheduler: true,
            enable_optimistic_scheduler: true,
            enable_hinted_scheduler: true,
            enable_monitor: true,
            enable_bounded_backlog: true,
            enable_vote_rebroadcast: true,
            enable_bootstrap_responder: true,
            default_peering_port: network_params.network.default_node_port,
            bootstrap_fraction_numerator: 1,
            receive_minimum: Amount::micronano(1),
            online_weight_minimum: Amount::nano(60_000_000),
            representative_vote_weight_minimum: Amount::nano(10),
            password_fanout: 1024,
            io_threads: max(parallelism, 4),
            network_threads: max(parallelism, 4) as u32,
            work_threads: max(parallelism, 4) as u32,
            opencl: Default::default(),
            background_threads: max(parallelism, 4) as u32,
            /* Use half available threads on the system for signature checking. The calling thread does checks as well, so these are extra worker threads */
            signature_checker_threads: (parallelism / 2) as u32,
            bootstrap_initiator_threads: 1,
            bootstrap_serving_threads: 1,
            allow_local_peers, // disable by default for live and test networks
            vote_minimum: Amount::nano(1000),
            vote_generator_delay: Duration::from_millis(100),
            unchecked_cutoff_time_s: 4 * 60 * 60, // 4 hours
            pow_sleep_interval_ns: 0,
            external_address: Ipv6Addr::UNSPECIFIED.to_string(),
            external_port: 0,
            use_memory_pools: true,
            bootstrap: Default::default(),
            bootstrap_server: Default::default(),
            confirming_set_batch_time: Duration::from_millis(250),
            max_work_generate_multiplier: 64_f64,
            max_queued_requests: 512,
            request_aggregator_threads: max(parallelism, 4) as u32,
            max_unchecked_blocks: 65536,
            rep_crawler_weight_minimum: Amount::decode_hex("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF")
                .unwrap(),
            work_peers: Vec::new(),
            secondary_work_peers: vec![Peer::new("127.0.0.1", 8076)],
            preconfigured_peers,
            preconfigured_representatives,
            callback_address: String::new(),
            callback_port: 0,
            callback_target: String::new(),
            websocket_config: WebsocketConfig::new(&network_params.network),
            lmdb_config: LmdbConfig::new(),
            optimistic_scheduler: OptimisticSchedulerConfig::new(),
            hinted_scheduler: if network_params.network.is_dev_network() {
                HintedSchedulerConfig::default_for_dev_network()
            } else {
                HintedSchedulerConfig::default()
            },
            priority_bucket: Default::default(),
            vote_cache: Default::default(),
            active_elections: Default::default(),
            rep_crawler_query_timeout: if network_params.network.is_dev_network() {
                Duration::from_secs(1)
            } else {
                Duration::from_secs(60)
            },
            block_processor: block_processor_cfg,
            block_processor_threads: max(2, parallelism / 2),
            vote_processor: VoteProcessorConfig::new(parallelism),
            tcp: if network_params.network.is_dev_network() {
                TcpConfig::for_dev_network()
            } else {
                Default::default()
            },
            request_aggregator: RequestAggregatorConfig::new(parallelism),
            message_processor: MessageProcessorConfig::new(parallelism),
            local_block_broadcaster: LocalBlockBroadcasterConfig::new(network),
            confirming_set: Default::default(),
            monitor: Default::default(),
            backlog_scan: Default::default(),
            bounded_backlog: Default::default(),
            network_duplicate_filter_size: 1024 * 1024,
            network_duplicate_filter_cutoff: 60,
            network: NetworkConfig {
                allow_local_peers,
                listening_port: peering_port.unwrap_or_default(),
                ..NetworkConfig::default_for(network)
            },
            confirmation_history_size: 2048,
            fork_cache_max_size: ForkCache::DEFAULT_MAX_LEN,
            fork_cache_max_forks_per_root: ForkCache::DEFAULT_MAX_FORKS_PER_ROOT,
            bootstrap_stale_threshold: BootstrapStaleElections::DEFAULT_STALE_THRESHOLD,
            vote_rebroadcaster_max_queue: VoteRebroadcastQueue::DEFAULT_MAX_QUEUE,
            rebroadcast_history: Default::default(),
            cps_limit: 0,
        }
    }

    pub fn new_test_instance() -> Self {
        Self::new(None, &DEV_NETWORK_PARAMS, 1)
    }

    pub fn rpc_callback_url(&self) -> Option<Url> {
        format!(
            "http://{}:{}{}",
            self.callback_address, self.callback_port, self.callback_target
        )
        .parse()
        .ok()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonitorConfig {
    pub interval: Duration,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TcpConfig {
    pub max_inbound_connections: usize,
    pub max_outbound_connections: usize,
    pub max_attempts: usize,
    pub max_attempts_per_ip: usize,
    pub connect_timeout: Duration,
}

impl TcpConfig {
    pub fn for_dev_network() -> Self {
        Self {
            max_inbound_connections: 128,
            max_outbound_connections: 128,
            max_attempts: 128,
            max_attempts_per_ip: 128,
            connect_timeout: Duration::from_secs(5),
        }
    }
}

impl Default for TcpConfig {
    fn default() -> Self {
        Self {
            max_inbound_connections: 2048,
            max_outbound_connections: 2048,
            max_attempts: 60,
            max_attempts_per_ip: 1,
            connect_timeout: Duration::from_secs(60),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let config = NodeConfig::default_for(NetworkType::NanoLiveNetwork, 2);
        assert_eq!(config.fork_cache_max_size, ForkCache::DEFAULT_MAX_LEN);
        assert_eq!(
            config.fork_cache_max_forks_per_root,
            ForkCache::DEFAULT_MAX_FORKS_PER_ROOT
        );
        assert_eq!(
            config.bootstrap_stale_threshold,
            BootstrapStaleElections::DEFAULT_STALE_THRESHOLD
        );
    }
}
