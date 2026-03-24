use super::{fork_cache_toml::ForkCacheToml, *};
use crate::config::NodeConfig;
use bounded_backlog_toml::BoundedBacklogToml;
use rsnano_types::{Account, Amount, Peer};
use serde::{Deserialize, Serialize};
use std::{str::FromStr, time::Duration};
use tcp_toml::TcpToml;

#[derive(Serialize, Deserialize, Default)]
pub struct NodeToml {
    pub allow_local_peers: Option<bool>,
    pub background_threads: Option<u32>,
    pub bandwidth_limit: Option<usize>,
    pub bandwidth_limit_burst_ratio: Option<f64>,
    pub bootstrap_bandwidth_burst_ratio: Option<f64>,
    pub bootstrap_bandwidth_limit: Option<usize>,
    pub bootstrap_fraction_numerator: Option<u32>,
    pub bootstrap_initiator_threads: Option<u32>,
    pub bootstrap_serving_threads: Option<u32>,
    pub confirming_set_batch_time: Option<u64>,
    pub enable_voting: Option<bool>,
    pub external_address: Option<String>,
    pub external_port: Option<u16>,
    pub io_threads: Option<usize>,
    pub max_queued_requests: Option<u32>,
    pub max_unchecked_blocks: Option<u32>,
    pub max_backlog: Option<u64>,
    pub max_work_generate_multiplier: Option<f64>,
    pub network_threads: Option<u32>,
    pub online_weight_minimum: Option<String>,
    pub password_fanout: Option<u32>,
    pub pow_sleep_interval: Option<i64>,
    pub preconfigured_peers: Option<Vec<String>>,
    pub preconfigured_representatives: Option<Vec<String>>,
    pub receive_minimum: Option<String>,
    pub rep_crawler_weight_minimum: Option<String>,
    pub representative_vote_weight_minimum: Option<String>,
    pub request_aggregator_threads: Option<u32>,
    pub signature_checker_threads: Option<u32>,
    pub unchecked_cutoff_time: Option<i64>,
    pub use_memory_pools: Option<bool>,
    pub vote_generator_delay: Option<u64>,
    pub vote_minimum: Option<String>,
    pub work_peers: Option<Vec<String>>,
    pub work_threads: Option<u32>,
    pub active_elections: Option<ActiveElectionsToml>,
    pub block_processor: Option<BlockProcessorToml>,
    pub bootstrap: Option<BootstrapToml>,
    pub bootstrap_server: Option<BootstrapServerToml>,
    pub experimental: Option<ExperimentalToml>,
    pub httpcallback: Option<HttpcallbackToml>,
    pub lmdb: Option<LmdbToml>,
    pub message_processor: Option<MessageProcessorToml>,
    pub monitor: Option<MonitorToml>,
    pub optimistic_scheduler: Option<OptimisticSchedulerToml>,
    pub hinted_scheduler: Option<HintedSchedulerToml>,
    pub priority_bucket: Option<PriorityBucketToml>,
    pub rep_crawler: Option<RepCrawlerToml>,
    pub request_aggregator: Option<RequestAggregatorToml>,
    pub vote_cache: Option<VoteCacheToml>,
    pub vote_processor: Option<VoteProcessorToml>,
    pub websocket: Option<WebsocketToml>,
    pub backlog_scan: Option<BacklogScanToml>,
    pub bounded_backlog: Option<BoundedBacklogToml>,
    pub tcp: Option<TcpToml>,
    pub network: Option<NetworkToml>,
    pub fork_cache: Option<ForkCacheToml>,
    pub vote_rebroadcaster: Option<VoteRebroadcasterToml>,
    pub peering_port: Option<u16>,
    pub cps_limit: Option<u32>,
}

impl NodeConfig {
    pub fn merge_toml(&mut self, toml: &NodeToml) {
        if let Some(allow_local_peers) = toml.allow_local_peers {
            self.allow_local_peers = allow_local_peers;
            self.network.allow_local_peers = allow_local_peers;
        }
        if let Some(background_threads) = toml.background_threads {
            self.background_threads = background_threads;
        }
        if let Some(limit) = toml.bandwidth_limit {
            self.network.limiter.generic_limit = limit;
        }
        if let Some(ratio) = toml.bandwidth_limit_burst_ratio {
            self.network.limiter.generic_burst_ratio = ratio;
        }
        if let Some(limit) = toml.bootstrap_bandwidth_limit {
            self.network.limiter.bootstrap_limit = limit;
        }
        if let Some(ratio) = toml.bootstrap_bandwidth_burst_ratio {
            self.network.limiter.bootstrap_burst_ratio = ratio;
        }
        if let Some(bootstrap_fraction_numerator) = toml.bootstrap_fraction_numerator {
            self.bootstrap_fraction_numerator = bootstrap_fraction_numerator;
        }
        if let Some(bootstrap_initiator_threads) = toml.bootstrap_initiator_threads {
            self.bootstrap_initiator_threads = bootstrap_initiator_threads;
        }
        if let Some(bootstrap_serving_threads) = toml.bootstrap_serving_threads {
            self.bootstrap_serving_threads = bootstrap_serving_threads;
        }
        if let Some(confirming_set_batch_time) = &toml.confirming_set_batch_time {
            self.confirming_set_batch_time = Duration::from_millis(*confirming_set_batch_time);
        }
        if let Some(enable_voting) = toml.enable_voting {
            self.enable_voting = enable_voting;
        }
        if let Some(opt) = &toml.optimistic_scheduler
            && let Some(enable) = opt.enable
        {
            self.enable_optimistic_scheduler = enable;
        }
        if let Some(external_address) = &toml.external_address {
            self.external_address = external_address.clone();
        }
        if let Some(external_port) = toml.external_port {
            self.external_port = external_port;
        }
        if let Some(io_threads) = toml.io_threads {
            self.io_threads = io_threads;
        }
        if let Some(max_queued_requests) = toml.max_queued_requests {
            self.max_queued_requests = max_queued_requests;
        }
        if let Some(max_unchecked_blocks) = toml.max_unchecked_blocks {
            self.max_unchecked_blocks = max_unchecked_blocks;
        }
        if let Some(max) = toml.max_backlog {
            self.bounded_backlog.max_backlog = max;
        }
        if let Some(max_work_generate_multiplier) = toml.max_work_generate_multiplier {
            self.max_work_generate_multiplier = max_work_generate_multiplier;
        }
        if let Some(network_threads) = toml.network_threads {
            self.network_threads = network_threads;
        }
        if let Some(online_weight_minimum) = &toml.online_weight_minimum {
            self.online_weight_minimum =
                Amount::decode_dec(online_weight_minimum).expect("Invalid online weight minimum");
        }
        if let Some(password_fanout) = toml.password_fanout {
            self.password_fanout = password_fanout;
        }
        if let Some(pow_sleep_interval_ns) = toml.pow_sleep_interval {
            self.pow_sleep_interval_ns = pow_sleep_interval_ns;
        }
        if let Some(preconfigured_peers) = &toml.preconfigured_peers {
            self.preconfigured_peers =
                Peer::parse_list(preconfigured_peers, self.default_peering_port);
        }
        if let Some(preconfigured_representatives) = &toml.preconfigured_representatives {
            self.preconfigured_representatives = preconfigured_representatives
                .iter()
                .map(|string| {
                    Account::parse(string)
                        .expect("Invalid representative")
                        .into()
                })
                .collect();
        }
        if let Some(receive_minimum) = &toml.receive_minimum {
            self.receive_minimum =
                Amount::decode_dec(receive_minimum).expect("Invalid receive minimum");
        }
        if let Some(rep_crawler) = &toml.rep_crawler
            && let Some(query_timeout) = rep_crawler.query_timeout
        {
            self.rep_crawler_query_timeout = Duration::from_millis(query_timeout);
        }
        if let Some(representative_vote_weight_minimum) = &toml.representative_vote_weight_minimum {
            self.representative_vote_weight_minimum =
                Amount::decode_dec(representative_vote_weight_minimum)
                    .expect("Invalid representative vote weight minimum");
        }
        if let Some(request_aggregator_threads) = toml.request_aggregator_threads {
            self.request_aggregator_threads = request_aggregator_threads;
        }
        if let Some(signature_checker_threads) = toml.signature_checker_threads {
            self.signature_checker_threads = signature_checker_threads;
        }
        if let Some(unchecked_cutoff_time_s) = toml.unchecked_cutoff_time {
            self.unchecked_cutoff_time_s = unchecked_cutoff_time_s;
        }
        if let Some(use_memory_pools) = toml.use_memory_pools {
            self.use_memory_pools = use_memory_pools;
        }
        if let Some(delay) = toml.vote_generator_delay {
            self.vote_generator_delay = Duration::from_millis(delay);
        }
        if let Some(vote_minimum) = &toml.vote_minimum {
            self.vote_minimum = Amount::decode_dec(vote_minimum).expect("Invalid vote minimum");
        }
        if let Some(work_peers) = &toml.work_peers {
            self.work_peers = work_peers
                .iter()
                .map(|string| Peer::from_str(string).expect("Invalid work peer"))
                .collect();
        }
        if let Some(work_threads) = toml.work_threads {
            self.work_threads = work_threads;
        }
        if let Some(cfg) = &toml.optimistic_scheduler {
            if let Some(enable) = cfg.enable {
                self.enable_optimistic_scheduler = enable;
            }
            self.optimistic_scheduler.merge_toml(cfg);
        }
        if let Some(cfg) = &toml.hinted_scheduler {
            if let Some(enable) = cfg.enable {
                self.enable_hinted_scheduler = enable;
            }
            self.hinted_scheduler.merge_toml(cfg);
        }
        if let Some(priority_bucket_toml) = &toml.priority_bucket {
            self.priority_bucket = priority_bucket_toml.into();
        }
        if let Some(boot_toml) = &toml.bootstrap {
            let config = &mut self.bootstrap;
            if let Some(enable) = &boot_toml.enable {
                config.enable = *enable;
            }
            if let Some(enable) = &boot_toml.enable_priorities {
                config.enable_priorities = *enable;
            }
            if let Some(enable) = &boot_toml.enable_dependency_walker {
                config.enable_dependency_walker = *enable;
            }
            if let Some(enable) = &boot_toml.enable_frontier_scan {
                config.enable_frontier_scan = *enable;
            }
            if let Some(account_sets) = &boot_toml.account_sets {
                config.candidate_accounts = account_sets.into();
            }
            if let Some(front) = &boot_toml.frontier_scan {
                config.merge_toml(front);
                if let Some(i) = front.max_pending {
                    config.max_pending_frontier_responses = i;
                }
            }
            if let Some(threshold) = boot_toml.block_processor_threshold {
                config.block_processor_theshold = threshold;
            }
            if let Some(limit) = boot_toml.database_rate_limit {
                config.database_rate_limit = limit;
            }
            if let Some(limit) = boot_toml.frontier_rate_limit {
                config.frontier_rate_limit = limit;
            }
            if let Some(database_warmup_ratio) = boot_toml.database_warmup_ratio {
                config.database_warmup_ratio = database_warmup_ratio;
            }
            if let Some(pull_count) = boot_toml.max_pull_count {
                config.max_pull_count = pull_count;
            }
            if let Some(limit) = boot_toml.channel_limit {
                config.channel_limit = limit;
            }
            if let Some(limit) = boot_toml.rate_limit {
                config.rate_limit = limit;
            }
            if let Some(timeout) = &boot_toml.request_timeout {
                config.request_timeout = Duration::from_millis(*timeout);
            }
            if let Some(throttle_wait) = &boot_toml.throttle_wait {
                config.throttle_wait = Duration::from_millis(*throttle_wait);
            }
            if let Some(throttle_coefficient) = boot_toml.throttle_coefficient {
                config.throttle_coefficient = throttle_coefficient;
            }
            if let Some(max) = boot_toml.max_requests {
                config.max_requests = max;
            }
            if let Some(percent) = boot_toml.optimistic_request_percentage {
                config.optimistic_request_percentage = percent;
            }
        }
        if let Some(toml) = &toml.bootstrap_server {
            self.bootstrap_server = toml.into();
            if let Some(enable) = toml.enable {
                self.enable_bootstrap_responder = enable;
            }
        }
        if let Some(websocket_config_toml) = &toml.websocket {
            self.websocket_config.merge_toml(websocket_config_toml);
        }
        if let Some(lmdb_config_toml) = &toml.lmdb {
            self.lmdb_config = lmdb_config_toml.into();
        }
        if let Some(vote_cache_toml) = &toml.vote_cache {
            self.vote_cache = vote_cache_toml.into();
        }
        if let Some(t) = &toml.block_processor {
            self.block_processor.merge_toml(t);
            if let Some(threads) = t.threads {
                self.block_processor_threads = threads;
            }
        }

        if let Some(i) = &toml.active_elections {
            if let Some(size) = i.size {
                self.active_elections.max_elections = size
            };
            if let Some(percentage) = i.hinted_limit_percentage {
                self.hinted_scheduler.hinted_limit_percentage = percentage;
            };
            if let Some(percentage) = i.optimistic_limit_percentage {
                self.optimistic_scheduler.optimistic_limit_percentage = percentage;
            };
            if let Some(size) = i.confirmation_history_size {
                self.confirmation_history_size = size;
            };
            if let Some(cache) = i.confirmation_cache {
                self.active_elections.confirmation_cache = cache;
            };
            if let Some(threshold) = i.bootstrap_stale_threshold {
                self.bootstrap_stale_threshold = Duration::from_secs(threshold as u64);
            }
        }
        if let Some(vote_processor_toml) = &toml.vote_processor {
            self.vote_processor.merge_toml(vote_processor_toml);
        }
        if let Some(request_aggregator_toml) = &toml.request_aggregator {
            self.request_aggregator.merge_toml(request_aggregator_toml);
        }
        if let Some(message_processor_toml) = &toml.message_processor {
            self.message_processor.merge_toml(message_processor_toml);
        }
        if let Some(cfg) = &toml.monitor {
            if let Some(enable) = cfg.enable {
                self.enable_monitor = enable;
            }
            if let Some(secs) = cfg.interval {
                self.monitor.interval = Duration::from_secs(secs);
            }
        }
        if let Some(rep_crawler_weight_minimum) = &toml.rep_crawler_weight_minimum {
            self.rep_crawler_weight_minimum = Amount::decode_dec(rep_crawler_weight_minimum)
                .expect("Invalid rep crawler weight minimum");
        }
        if let Some(httpcallback) = &toml.httpcallback {
            if let Some(address) = &httpcallback.address {
                self.callback_address = address.clone();
            }
            if let Some(port) = &httpcallback.port {
                self.callback_port = *port;
            }
            if let Some(target) = &httpcallback.target {
                self.callback_target = target.clone();
            }
        }
        if let Some(toml) = &toml.backlog_scan {
            self.backlog_scan.merge_toml(toml);
        }
        self.bounded_backlog.merge_toml(toml);
        if let Some(toml) = &toml.bounded_backlog
            && let Some(enable) = toml.enable
        {
            self.enable_bounded_backlog = enable;
        }

        if let Some(toml) = &toml.tcp {
            self.tcp.merge_toml(toml);
        }

        if let Some(network) = &toml.network {
            if let Some(i) = network.peer_reachout {
                self.network.peer_reachout = Duration::from_millis(i as u64);
            }
            if let Some(i) = network.cached_peer_reachout {
                self.network.cached_peer_reachout = Duration::from_millis(i as u64);
            }
            if let Some(i) = network.max_peers_per_ip {
                self.network.max_peers_per_ip = i;
            }
            if let Some(i) = network.max_peers_per_subnetwork {
                self.network.max_peers_per_subnetwork = i;
            }
            if let Some(i) = network.duplicate_filter_size {
                self.network_duplicate_filter_size = i;
            }
            if let Some(i) = network.duplicate_filter_cutoff {
                self.network_duplicate_filter_cutoff = i as u64;
            }
            if let Some(i) = network.minimum_fanout {
                self.network.minimum_fanout = i;
            }
        }

        if let Some(toml) = &toml.fork_cache {
            if let Some(i) = toml.max_size {
                self.fork_cache_max_size = i;
            }
            if let Some(i) = toml.max_forks_per_root {
                self.fork_cache_max_forks_per_root = i;
            }
        }

        if let Some(toml) = &toml.vote_rebroadcaster {
            if let Some(i) = toml.enable {
                self.enable_vote_rebroadcast = i;
            }
            if let Some(i) = toml.max_queue {
                self.vote_rebroadcaster_max_queue = i;
            }
            if let Some(i) = toml.max_history {
                self.rebroadcast_history.max_blocks_per_rep = i;
            }
            if let Some(i) = toml.max_representatives {
                self.rebroadcast_history.max_representatives = i;
            }
            if let Some(i) = toml.rebroadcast_threshold {
                self.rebroadcast_history.rebroadcast_min_gap = Duration::from_millis(i);
            }
        }

        if let Some(port) = toml.peering_port {
            self.network.listening_port = port;
        }

        if let Some(limit) = toml.cps_limit {
            self.cps_limit = limit;
        }
    }
}

impl From<&NodeConfig> for NodeToml {
    fn from(config: &NodeConfig) -> Self {
        Self {
            allow_local_peers: Some(config.allow_local_peers),
            background_threads: Some(config.background_threads),
            bandwidth_limit: Some(config.network.limiter.generic_limit),
            bandwidth_limit_burst_ratio: Some(config.network.limiter.generic_burst_ratio),
            bootstrap_bandwidth_limit: Some(config.network.limiter.bootstrap_limit),
            bootstrap_bandwidth_burst_ratio: Some(config.network.limiter.bootstrap_burst_ratio),
            bootstrap_fraction_numerator: Some(config.bootstrap_fraction_numerator),
            bootstrap_initiator_threads: Some(config.bootstrap_initiator_threads),
            bootstrap_serving_threads: Some(config.bootstrap_serving_threads),
            confirming_set_batch_time: Some(config.confirming_set_batch_time.as_millis() as u64),
            enable_voting: Some(config.enable_voting),
            external_address: Some(config.external_address.clone()),
            external_port: Some(config.external_port),
            io_threads: Some(config.io_threads),
            max_queued_requests: Some(config.max_queued_requests),
            max_unchecked_blocks: Some(config.max_unchecked_blocks),
            max_backlog: Some(config.bounded_backlog.max_backlog),
            max_work_generate_multiplier: Some(config.max_work_generate_multiplier),
            network_threads: Some(config.network_threads),
            online_weight_minimum: Some(config.online_weight_minimum.to_string_dec()),
            password_fanout: Some(config.password_fanout),
            pow_sleep_interval: Some(config.pow_sleep_interval_ns),
            preconfigured_peers: Some(
                config
                    .preconfigured_peers
                    .iter()
                    .map(|p| p.to_string())
                    .collect(),
            ),
            preconfigured_representatives: Some(
                config
                    .preconfigured_representatives
                    .iter()
                    .map(|pk| Account::from(pk).encode_account())
                    .collect(),
            ),
            receive_minimum: Some(config.receive_minimum.to_string_dec()),
            rep_crawler_weight_minimum: Some(config.rep_crawler_weight_minimum.to_string_dec()),
            representative_vote_weight_minimum: Some(
                config.representative_vote_weight_minimum.to_string_dec(),
            ),
            request_aggregator_threads: Some(config.request_aggregator_threads),
            signature_checker_threads: Some(config.signature_checker_threads),
            unchecked_cutoff_time: Some(config.unchecked_cutoff_time_s),
            use_memory_pools: Some(config.use_memory_pools),
            vote_generator_delay: Some(config.vote_generator_delay.as_millis() as u64),
            vote_minimum: Some(config.vote_minimum.to_string_dec()),
            work_peers: Some(
                config
                    .work_peers
                    .iter()
                    .map(|peer| peer.to_string())
                    .collect(),
            ),
            work_threads: Some(config.work_threads),
            optimistic_scheduler: Some(OptimisticSchedulerToml {
                enable: Some(config.enable_optimistic_scheduler),
                gap_threshold: Some(config.optimistic_scheduler.gap_threshold),
                max_size: Some(config.optimistic_scheduler.max_size),
            }),
            hinted_scheduler: Some(HintedSchedulerToml {
                enable: Some(config.enable_hinted_scheduler),
                hinting_threshold: Some(config.hinted_scheduler.hinting_threshold_percent),
                check_interval: Some(config.hinted_scheduler.check_interval.as_millis() as u64),
                block_cooldown: Some(config.hinted_scheduler.block_cooldown.as_millis() as u64),
                vacancy_threshold: Some(config.hinted_scheduler.vacancy_threshold_percent),
            }),
            priority_bucket: Some((&config.priority_bucket).into()),
            bootstrap: Some((&config.bootstrap).into()),
            bootstrap_server: Some(config.into()),
            websocket: Some((&config.websocket_config).into()),
            lmdb: Some((&config.lmdb_config).into()),
            vote_cache: Some((&config.vote_cache).into()),
            block_processor: Some(config.into()),
            active_elections: Some(config.into()),
            vote_processor: Some((&config.vote_processor).into()),
            request_aggregator: Some((&config.request_aggregator).into()),
            message_processor: Some((&config.message_processor).into()),
            monitor: Some(MonitorToml {
                enable: Some(config.enable_monitor),
                interval: Some(config.monitor.interval.as_secs()),
            }),
            httpcallback: Some(config.into()),
            rep_crawler: Some(config.into()),
            experimental: Some(config.into()),
            backlog_scan: Some((&config.backlog_scan).into()),
            bounded_backlog: Some(config.into()),
            tcp: Some((&config.tcp).into()),
            network: Some(config.into()),
            fork_cache: Some(config.into()),
            vote_rebroadcaster: Some(config.into()),
            peering_port: Some(config.network.listening_port),
            cps_limit: Some(config.cps_limit),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{block_processing::ProcessQueueConfig, config::toml::AccountSetsToml};

    #[test]
    fn merge_bootstrap_ascending_toml() {
        let sets_toml = AccountSetsToml {
            blocking_max: Some(200),
            consideration_count: Some(201),
            cooldown: Some(203),
            priorities_max: Some(204),
            blocking_decay: None,
        };

        let ascending_toml = BootstrapToml {
            enable: Some(false),
            enable_dependency_walker: Some(false),
            enable_frontier_scan: Some(false),
            block_processor_threshold: Some(100),
            database_rate_limit: Some(101),
            max_pull_count: Some(102),
            channel_limit: Some(103),
            rate_limit: Some(200),
            throttle_coefficient: Some(104),
            throttle_wait: Some(105),
            request_timeout: Some(106),
            max_requests: Some(107),
            optimistic_request_percentage: Some(42),
            database_warmup_ratio: Some(108),
            account_sets: Some(sets_toml),
            frontier_scan: None,
            enable_priorities: None,
            frontier_rate_limit: None,
        };

        let toml = NodeToml {
            bootstrap: Some(ascending_toml),
            ..Default::default()
        };

        let mut cfg = NodeConfig::new_test_instance();
        cfg.merge_toml(&toml);

        let ascending = &cfg.bootstrap;
        assert_eq!(ascending.enable, false);
        assert_eq!(ascending.enable_dependency_walker, false);
        assert_eq!(ascending.enable_frontier_scan, false);
        assert_eq!(ascending.block_processor_theshold, 100);
        assert_eq!(ascending.database_rate_limit, 101);
        assert_eq!(ascending.max_pull_count, 102);
        assert_eq!(ascending.channel_limit, 103);
        assert_eq!(ascending.rate_limit, 200);
        assert_eq!(ascending.throttle_coefficient, 104);
        assert_eq!(ascending.throttle_wait, Duration::from_millis(105));
        assert_eq!(ascending.request_timeout, Duration::from_millis(106));
        assert_eq!(ascending.max_requests, 107);
        assert_eq!(ascending.optimistic_request_percentage, 42);
        assert_eq!(ascending.database_warmup_ratio, 108);

        let sets = &cfg.bootstrap.candidate_accounts;
        assert_eq!(sets.blocking_max, 200);
        assert_eq!(sets.consideration_count, 201);
        assert_eq!(sets.cooldown, Duration::from_millis(203));
        assert_eq!(sets.priorities_max, 204);
    }

    #[test]
    fn create_bootstrap_ascending_toml() {
        let cfg = NodeConfig::new_test_instance();
        let toml = NodeToml::from(&cfg);
        let ascending_toml = toml.bootstrap.as_ref().unwrap();
        assert_eq!(ascending_toml.enable, Some(true));
        assert_eq!(ascending_toml.enable_frontier_scan, Some(true));
        assert_eq!(ascending_toml.enable_dependency_walker, Some(true));
        assert_eq!(ascending_toml.block_processor_threshold, Some(1000));
        assert_eq!(ascending_toml.database_rate_limit, Some(256));
        assert_eq!(ascending_toml.database_warmup_ratio, Some(10));
        assert_eq!(ascending_toml.max_pull_count, Some(128));
        assert_eq!(ascending_toml.channel_limit, Some(16));
        assert_eq!(ascending_toml.throttle_coefficient, Some(1024 * 8));
        assert_eq!(ascending_toml.throttle_wait, Some(100));
        assert_eq!(ascending_toml.request_timeout, Some(15000));
        assert_eq!(ascending_toml.max_requests, Some(1024));
        assert_eq!(ascending_toml.optimistic_request_percentage, Some(75));

        let sets_toml = ascending_toml.account_sets.as_ref().unwrap();
        assert_eq!(sets_toml.consideration_count, Some(4));
        assert_eq!(sets_toml.priorities_max, Some(1024 * 256));
        assert_eq!(sets_toml.blocking_max, Some(1024 * 256));
        assert_eq!(sets_toml.cooldown, Some(3000));
    }

    #[test]
    fn merge_node_config() {
        let toml = NodeToml {
            fork_cache: Some(ForkCacheToml {
                max_size: Some(222),
                max_forks_per_root: Some(22),
            }),
            active_elections: Some(ActiveElectionsToml {
                bootstrap_stale_threshold: Some(42),
                ..Default::default()
            }),
            cps_limit: Some(42),
            block_processor: Some(BlockProcessorToml {
                threads: Some(42),
                max_peer_queue: Some(43),
                max_system_queue: Some(44),
                priority_bootstrap: Some(45),
                priority_live: Some(46),
                priority_local: Some(47),
            }),
            ..Default::default()
        };

        let mut cfg = NodeConfig::new_test_instance();
        cfg.merge_toml(&toml);

        assert_eq!(cfg.fork_cache_max_size, 222);
        assert_eq!(cfg.fork_cache_max_forks_per_root, 22);
        assert_eq!(cfg.bootstrap_stale_threshold, Duration::from_secs(42));
        assert_eq!(cfg.cps_limit, 42);
        assert_eq!(cfg.block_processor_threads, 42);
        assert_eq!(cfg.block_processor.max_peer_queue, 43);
        assert_eq!(cfg.block_processor.max_system_queue, 44);
        assert_eq!(cfg.block_processor.priority_bootstrap, 45);
        assert_eq!(cfg.block_processor.priority_live, 46);
        assert_eq!(cfg.block_processor.priority_local, 47);
    }

    #[test]
    fn convert_config_to_toml() {
        let config = NodeConfig {
            bootstrap_stale_threshold: Duration::from_secs(42),
            cps_limit: 42,
            block_processor_threads: 43,
            block_processor: ProcessQueueConfig {
                max_peer_queue: 44,
                max_system_queue: 45,
                priority_live: 46,
                priority_bootstrap: 47,
                priority_local: 48,
                priority_system: 49,
                batch_size: 50,
            },
            ..NodeConfig::new_test_instance()
        };

        let toml = NodeToml::from(&config);

        assert_eq!(
            toml.active_elections
                .as_ref()
                .unwrap()
                .bootstrap_stale_threshold,
            Some(42)
        );
        assert_eq!(toml.cps_limit, Some(42));

        let block_proc = toml.block_processor.unwrap();
        assert_eq!(block_proc.threads, Some(43));
        assert_eq!(block_proc.max_peer_queue, Some(44));
        assert_eq!(block_proc.max_system_queue, Some(45));
        assert_eq!(block_proc.priority_live, Some(46));
        assert_eq!(block_proc.priority_bootstrap, Some(47));
        assert_eq!(block_proc.priority_local, Some(48));
    }
}
