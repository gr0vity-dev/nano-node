use std::{
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
};

use rsnano_ledger::{AnySet, ConfirmedSet, Ledger};
use rsnano_nullable_clock::SteadyClock;
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::{Account, AccountInfo, BlockHash, ConfirmationHeightInfo};
use rsnano_utils::{
    container_info::ContainerInfo,
    stats::{DetailType, StatType, Stats, StatsCollection, StatsSource},
};

use super::{PriorityBucketConfig, prio_bucket_count};
use crate::{
    block_processing::backlog_scan::UnconfirmedInfo,
    consensus::{
        AecService,
        election_schedulers::priority::{
            BucketInsertError, Eviction, priority_buckets::PriorityBuckets,
        },
    },
};

pub struct PriorityScheduler {
    stopped: Mutex<bool>,
    condition: Condvar,
    stats: Arc<Stats>,
    buckets: Mutex<PriorityBuckets>,
    thread: Mutex<Option<JoinHandle<()>>>,
    clock: Arc<SteadyClock>,
    aec: Arc<AecService>,
    ledger: Arc<Ledger>,
    activate_listener: OutputListenerMt<Account>,
}

impl PriorityScheduler {
    pub(crate) fn new(
        config: PriorityBucketConfig,
        stats: Arc<Stats>,
        active_elections: Arc<AecService>,
        ledger: Arc<Ledger>,
        clock: Arc<SteadyClock>,
    ) -> Self {
        let buckets = PriorityBuckets::new(prio_bucket_count(), config);

        Self {
            thread: Mutex::new(None),
            stopped: Mutex::new(false),
            condition: Condvar::new(),
            buckets: Mutex::new(buckets),
            stats,
            ledger,
            clock,
            aec: active_elections,
            activate_listener: Default::default(),
        }
    }

    pub fn track_activate(&self) -> Arc<OutputTrackerMt<Account>> {
        self.activate_listener.track()
    }

    pub fn stop(&self) {
        *self.stopped.lock().unwrap() = true;
        self.condition.notify_all();
        let handle = self.thread.lock().unwrap().take();
        if let Some(handle) = handle {
            handle.join().unwrap();
        }
    }

    pub fn notify(&self) {
        self.condition.notify_all();
    }

    pub fn contains(&self, hash: &BlockHash) -> bool {
        self.buckets.lock().unwrap().contains(hash)
    }

    pub fn activate(&self, any: &impl AnySet, account: &Account) {
        if self.activate_listener.is_tracked() {
            self.activate_listener.emit(*account);
        }
        debug_assert!(!account.is_zero());
        if let Some(account_info) = any.get_account(account) {
            let conf_info = any.confirmed().get_conf_info(account).unwrap_or_default();

            if conf_info.height < account_info.block_count {
                self.activate_with_info(any, &account_info, &conf_info);
                return;
            }
        };

        self.stats
            .inc(StatType::ElectionScheduler, DetailType::ActivateSkip);
    }

    pub fn activate_batch(&self, unconfirmed: &[UnconfirmedInfo]) {
        let any = self.ledger.any();
        for info in unconfirmed {
            self.activate_with_info(&any, &info.account_info, &info.conf_info);
        }
    }

    pub fn activate_with_info(
        &self,
        any: &impl AnySet,
        account_info: &AccountInfo,
        conf_info: &ConfirmationHeightInfo,
    ) {
        debug_assert!(conf_info.frontier != account_info.head);

        let next_unconfirmed_hash = match conf_info.height {
            0 => account_info.open_block,
            _ => {
                match any.block_successor(&conf_info.frontier) {
                    Some(h) => h,
                    None => {
                        // This can happen if the bounded backlog did a rollback
                        return;
                    }
                }
            }
        };

        let Some(block) = any.get_block(&next_unconfirmed_hash) else {
            return;
        };

        if !any.dependencies_confirmed(&block) {
            self.stats
                .inc(StatType::ElectionScheduler, DetailType::ActivateFailed);
            return;
        }

        #[cfg(feature = "ledger_snapshots")]
        if any.is_forked(&block.qualified_root()) {
            self.stats
                .inc(StatType::ElectionScheduler, DetailType::ActivateFailed);
            return;
        }

        let priority = any.block_priority(&block);

        let insert_result = self.buckets.lock().unwrap().insert(priority, block);

        match insert_result {
            Ok(Eviction::None) => {}
            Ok(Eviction::Evicted) => {
                self.stats
                    .inc(StatType::ElectionScheduler, DetailType::Evicted);
            }
            Err(BucketInsertError::Duplicate) => {
                self.stats
                    .inc(StatType::ElectionScheduler, DetailType::Duplicate);
            }
            Err(BucketInsertError::PriorityTooLow) => {
                self.stats
                    .inc(StatType::ElectionScheduler, DetailType::ActivateFull);
            }
        }

        if insert_result.is_ok() {
            self.stats
                .inc(StatType::ElectionScheduler, DetailType::Activated);
            self.condition.notify_all();
        }
    }

    pub fn len(&self) -> usize {
        self.buckets.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn run(&self) {
        let mut stopped = self.stopped.lock().unwrap();
        while !*stopped {
            stopped = self
                .condition
                .wait_while(stopped, |s| !*s && !self.predicate())
                .unwrap();

            if !*stopped {
                drop(stopped);
                self.run_one();
                stopped = self.stopped.lock().unwrap();
            }
        }
    }

    fn predicate(&self) -> bool {
        let buckets = self.buckets.lock().unwrap();
        self.aec.check_vacancy(&*buckets)
    }

    fn run_one(&self) {
        self.stats
            .inc(StatType::ElectionScheduler, DetailType::Loop);

        let now = self.clock.now();
        let mut buckets = self.buckets.lock().unwrap();
        self.aec.refill(&mut *buckets, now);
    }

    pub fn container_info(&self) -> ContainerInfo {
        let mut bucket_infos = ContainerInfo::builder();

        for (id, bucket) in self.buckets.lock().unwrap().iter().enumerate() {
            bucket_infos = bucket_infos.leaf(id.to_string(), bucket.len(), 0);
        }

        ContainerInfo::builder()
            .node("blocks", bucket_infos.finish())
            .finish()
    }
}

impl Drop for PriorityScheduler {
    fn drop(&mut self) {
        // Thread must be stopped before destruction
        debug_assert!(self.thread.lock().unwrap().is_none());
    }
}

pub trait PrioritySchedulerExt {
    fn start(&self);
}

impl PrioritySchedulerExt for Arc<PriorityScheduler> {
    fn start(&self) {
        debug_assert!(self.thread.lock().unwrap().is_none());

        let self_l = Arc::clone(self);
        *self.thread.lock().unwrap() = Some(
            std::thread::Builder::new()
                .name("Sched Priority".to_string())
                .spawn(Box::new(move || {
                    self_l.run();
                }))
                .unwrap(),
        );
    }
}

impl StatsSource for PriorityScheduler {
    fn collect_stats(&self, result: &mut StatsCollection) {
        let guard = self.buckets.lock().unwrap();
        guard.bucket_stats.collect_stats(result);
        guard.collect_stats(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aec_fact_processor::AecFactProcessor;
    use crate::block_processing::{BlockContext, BlockProcessorQueue, ProcessQueueConfig};
    use crate::bootstrap::bootstrapper::Bootstrapper;
    use crate::config::{NetworkParams, NodeConfig};
    use crate::consensus::{
        ActiveElectionsConfig, AecFact, AecForkInserter, AecService, AecVoter,
        BootstrapElectionActivator, CpsLimiter, LocalVoteHistory, LocalVotesRemover, VoteApplier,
        VoteBroadcaster, VoteCache, VoteCacheProcessor, VoteGenerators, VoteProcessor,
        VoteProcessorConfig, VoteProcessorExt, VoteProcessorQueue, VoteRebroadcastQueue,
        WinnerBlockBroadcaster,
        election::ElectionBehavior,
        election::{ConfirmationType, ConfirmedElection},
        election_schedulers::priority::PriorityBucketConfig,
    };
    use crate::recently_cemented_inserter::RecentlyCementedInserter;
    use crate::representatives::{OnlineReps, RepCrawler};
    use crate::transport::{
        MessageFlooder, MessageSender,
        keepalive::{KeepaliveMessageFactory, KeepalivePublisher},
    };
    use crate::utils::{BackpressureEventProcessor, spawn_backpressure_processor};
    use crate::wallets::WalletRepresentatives;
    use bounded_vec_deque::BoundedVecDeque;
    use rsnano_ledger::{
        BlockSource, CementingObserver, Ledger, LedgerBuilder, LedgerSet, RepWeightCache,
        test_helpers::UnsavedBlockLatticeBuilder,
    };
    use rsnano_messages::NetworkFilter;
    use rsnano_network::{ChannelId, Network, PeerConnector};
    use rsnano_nullable_clock::SteadyClock;
    use rsnano_store_lmdb::{LmdbConfig, SyncStrategy};
    use rsnano_types::{Amount, Block, BlockHash, NetworkType, Peer, PrivateKey, WalletId};
    use rsnano_utils::{CancellationToken, sync::backpressure_channel, ticker::Tickable};
    use rsnano_wallet::Wallets;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicBool, AtomicU64, Ordering},
        sync::mpsc::TryRecvError,
        sync::{Arc, Mutex, RwLock},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    static TEST_LEDGER_DIR_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn pressure_activation_uses_recomputed_ledger_priority_for_aec_selection() {
        let fixture = Fixture::new(1, PriorityBucketConfig::default());
        let low_account = PrivateKey::from(1);
        let high_account = PrivateKey::from(2);

        let high_open = fixture.process_open(&high_account, 2);
        let low_open = fixture.process_open(&low_account, 1);

        fixture.activate(low_account.account());
        fixture.activate(high_account.account());

        assert_eq!(fixture.scheduler.len(), 2);

        fixture.scheduler.run_one();

        assert_eq!(fixture.aec.len(), 1);
        assert_eq!(fixture.aec.count_by_behavior(ElectionBehavior::Priority), 1);
        assert!(fixture.aec.is_active_hash(&high_open.hash()));
        assert!(!fixture.aec.is_active_hash(&low_open.hash()));
    }

    #[test]
    fn pressure_activation_rejects_block_until_dependencies_are_confirmed() {
        let fixture = Fixture::new(1, PriorityBucketConfig::default());
        let account = PrivateKey::from(1);
        let open = fixture.process_open_without_confirmed_send(&account, 1);

        fixture.activate(account.account());

        assert_eq!(fixture.scheduler.len(), 0);

        fixture.scheduler.run_one();

        assert_eq!(fixture.aec.len(), 0);
        assert!(!fixture.aec.is_active_hash(&open.hash()));
    }

    #[test]
    fn pressure_priority_bucket_keeps_stronger_candidate_when_full() {
        let fixture = Fixture::new(1, PriorityBucketConfig { max_blocks: 1 });
        let weaker_account = PrivateKey::from(1);
        let stronger_account = PrivateKey::from(2);

        let stronger_open = fixture.process_open(&stronger_account, 2);
        let weaker_open = fixture.process_open(&weaker_account, 1);

        fixture.activate(weaker_account.account());
        fixture.activate(stronger_account.account());

        assert_eq!(fixture.scheduler.len(), 1);
        assert!(!fixture.scheduler.contains(&weaker_open.hash()));
        assert!(fixture.scheduler.contains(&stronger_open.hash()));
    }

    #[test]
    fn pressure_live_synced_pipeline_reports_comparable_stage_timings() {
        let test_dir = TestLedgerDir::new();
        let ledger = Arc::new(create_real_lmdb_ledger(&test_dir));
        let queue = BlockProcessorQueue::new(ProcessQueueConfig {
            batch_size: 64,
            ..Default::default()
        });
        let aec = Arc::new(AecService::new_null());
        let scheduler = PriorityScheduler::new(
            PriorityBucketConfig::default(),
            Arc::new(Stats::default()),
            aec.clone(),
            ledger.clone(),
            Arc::new(SteadyClock::new_null()),
        );
        let mut lattice = UnsavedBlockLatticeBuilder::with_stub_work();

        for i in 0..64 {
            let account = PrivateKey::from(i + 1);
            let send = lattice.genesis().send(&account, 1);
            let open = lattice.account(&account).receive(&send);
            ledger.process_one(&send).unwrap();
            ledger.confirm(send.hash());
            assert!(queue.push(BlockContext::new(
                open,
                BlockSource::Live,
                ChannelId::LOOPBACK,
            )));
        }

        let dequeue_start = Instant::now();
        let batch = queue.pop_blocking().unwrap();
        let dequeue_elapsed = dequeue_start.elapsed();

        let process_start = Instant::now();
        let results =
            ledger.process_batch(batch.iter().map(|context| (&context.block, context.source)));
        let process_elapsed = process_start.elapsed();
        let saved_blocks = results
            .iter()
            .map(|result| result.saved_block.clone().unwrap())
            .collect::<Vec<_>>();

        let schedule_start = Instant::now();
        for block in &saved_blocks {
            scheduler.activate(&ledger.any(), &block.account());
        }
        scheduler.run_one();
        let schedule_elapsed = schedule_start.elapsed();

        let confirmation_hashes = saved_blocks
            .iter()
            .map(|block| block.hash())
            .collect::<Vec<_>>();
        let stopped = AtomicBool::new(false);
        let mut observer = TimingCementingObserver::default();
        let confirm_start = Instant::now();
        ledger.confirm_batch(confirmation_hashes.iter(), &stopped, 1024, &mut observer);
        let confirm_elapsed = confirm_start.elapsed();

        assert_eq!(batch.len(), 64);
        assert_eq!(results.len(), 64);
        assert!(results.iter().all(|result| result.status.is_ok()));
        assert_eq!(queue.total_queue_len(), 0);
        assert_eq!(aec.len(), 64);
        assert!(observer.failed.is_empty());
        assert!(observer.already_confirmed.is_empty());
        assert!(
            confirmation_hashes
                .iter()
                .all(|hash| ledger.confirmed().block_exists(hash))
        );

        eprintln!(
            "live_synced_pipeline blocks={} dequeue_us={} process_us={} schedule_us={} confirm_us={} first_limiter={}",
            confirmation_hashes.len(),
            dequeue_elapsed.as_micros(),
            process_elapsed.as_micros(),
            schedule_elapsed.as_micros(),
            confirm_elapsed.as_micros(),
            first_limiter([
                ("dequeue", dequeue_elapsed),
                ("process_batch", process_elapsed),
                ("schedule", schedule_elapsed),
                ("confirm_batch", confirm_elapsed),
            ])
        );
    }

    #[test]
    fn pressure_live_synced_pipeline_backlog_grows_before_slowest_stage() {
        const BATCH_SIZE: usize = 64;
        const ARRIVALS_PER_WINDOW: usize = BATCH_SIZE * 2;
        const WINDOWS: usize = 3;

        let test_dir = TestLedgerDir::new();
        let ledger = Arc::new(create_real_lmdb_ledger(&test_dir));
        let queue = BlockProcessorQueue::new(ProcessQueueConfig {
            batch_size: BATCH_SIZE,
            ..Default::default()
        });
        let aec = Arc::new(AecService::new_null());
        let scheduler = PriorityScheduler::new(
            PriorityBucketConfig::default(),
            Arc::new(Stats::default()),
            aec.clone(),
            ledger.clone(),
            Arc::new(SteadyClock::new_null()),
        );
        let mut lattice = UnsavedBlockLatticeBuilder::with_stub_work();
        let stopped = AtomicBool::new(false);
        let mut observer = TimingCementingObserver::default();
        let mut process_elapsed = Duration::ZERO;
        let mut schedule_elapsed = Duration::ZERO;
        let mut confirm_elapsed = Duration::ZERO;
        let mut processed_total = 0;
        let mut confirmed_total = 0;
        let mut backlog_after_windows = Vec::new();

        for window in 0..WINDOWS {
            for offset in 0..ARRIVALS_PER_WINDOW {
                let account_index = window * ARRIVALS_PER_WINDOW + offset + 1;
                push_live_open(&ledger, &queue, &mut lattice, account_index);
            }

            let batch = queue.pop_blocking().unwrap();

            let process_start = Instant::now();
            let results =
                ledger.process_batch(batch.iter().map(|context| (&context.block, context.source)));
            process_elapsed += process_start.elapsed();
            processed_total += results.len();
            assert_eq!(results.len(), BATCH_SIZE);
            assert!(results.iter().all(|result| result.status.is_ok()));

            let saved_blocks = results
                .iter()
                .map(|result| result.saved_block.clone().unwrap())
                .collect::<Vec<_>>();

            let schedule_start = Instant::now();
            for block in &saved_blocks {
                scheduler.activate(&ledger.any(), &block.account());
            }
            scheduler.run_one();
            schedule_elapsed += schedule_start.elapsed();

            let confirmation_hashes = saved_blocks
                .iter()
                .map(|block| block.hash())
                .collect::<Vec<_>>();
            let confirm_start = Instant::now();
            ledger.confirm_batch(confirmation_hashes.iter(), &stopped, 1024, &mut observer);
            confirm_elapsed += confirm_start.elapsed();
            confirmed_total += confirmation_hashes.len();
            assert!(
                confirmation_hashes
                    .iter()
                    .all(|hash| ledger.confirmed().block_exists(hash))
            );

            backlog_after_windows.push(queue.total_queue_len());
        }

        assert_eq!(processed_total, WINDOWS * BATCH_SIZE);
        assert_eq!(confirmed_total, WINDOWS * BATCH_SIZE);
        assert_eq!(
            backlog_after_windows,
            vec![BATCH_SIZE, BATCH_SIZE * 2, BATCH_SIZE * 3]
        );
        assert_eq!(queue.total_queue_len(), WINDOWS * BATCH_SIZE);
        assert_eq!(aec.len() + scheduler.len(), WINDOWS * BATCH_SIZE);
        assert!(aec.len() < WINDOWS * BATCH_SIZE);
        assert!(scheduler.len() > 0);
        assert!(observer.failed.is_empty());
        assert!(observer.already_confirmed.is_empty());

        eprintln!(
            "live_synced_backlog windows={} arrivals_per_window={} serviced_per_window={} backlog_after_windows={:?} process_us={} schedule_us={} confirm_us={} first_limiter={}",
            WINDOWS,
            ARRIVALS_PER_WINDOW,
            BATCH_SIZE,
            backlog_after_windows,
            process_elapsed.as_micros(),
            schedule_elapsed.as_micros(),
            confirm_elapsed.as_micros(),
            first_limiter([
                ("process_batch", process_elapsed),
                ("schedule", schedule_elapsed),
                ("confirm_batch", confirm_elapsed),
            ])
        );
    }

    #[test]
    fn pressure_full_local_confirmation_path_reports_one_block_timing() {
        let mut fixture = FullConfirmationFixture::new(1);

        let report = fixture.process_and_confirm_live_blocks(1);

        assert_eq!(report.blocks, 1);
        assert_eq!(report.confirmed, 1);
        assert_eq!(
            report.non_final_vote_processed_events + report.final_vote_processed_events,
            2
        );
        assert_eq!(report.non_final_vote_ok_counts.iter().sum::<usize>(), 1);
        assert_eq!(report.final_vote_ok_counts.iter().sum::<usize>(), 1);
        assert_eq!(fixture.confirming_set.len(), 0);

        eprintln!("{}", report.format("full_local_confirmation_one"));
    }

    #[test]
    fn pressure_full_local_confirmation_path_reports_256_block_batch_timing() {
        let mut fixture = FullConfirmationFixture::new(256);

        let report = fixture.process_and_confirm_live_blocks(256);

        assert_eq!(report.blocks, 256);
        assert_eq!(report.confirmed, 256);
        assert_eq!(report.non_final_vote_ok_counts.iter().sum::<usize>(), 256);
        assert_eq!(report.final_vote_ok_counts.iter().sum::<usize>(), 256);
        assert_eq!(fixture.confirming_set.len(), 0);

        eprintln!("{}", report.format("full_local_confirmation_256"));
    }

    #[test]
    #[ignore = "pressure benchmark; run manually with --ignored --nocapture"]
    fn pressure_aec_fact_processor_thread_dispatch_reports_10k_across_10_peers_timing() {
        let mut fixture = AecFactDispatchFixture::new(10_000);

        let report = fixture.dispatch_confirmed_elections_through_backpressure_thread(10_000, 10);

        assert_eq!(report.blocks, 10_000);
        assert_eq!(report.peers, 10);
        assert_eq!(report.confirmed, 10_000);
        assert_eq!(fixture.confirming_set.len(), 0);

        eprintln!("{}", report.format("aec_fact_thread_dispatch_10k_10_peers"));
    }

    /* Test helpers */

    struct FullConfirmationFixture {
        ledger: Arc<Ledger>,
        queue: Arc<BlockProcessorQueue>,
        scheduler: PriorityScheduler,
        aec: Arc<AecService>,
        aec_fact_processor: AecFactProcessor,
        vote_generators: Option<Arc<VoteGenerators>>,
        vote_processor: Option<Arc<VoteProcessor>>,
        confirming_set: Arc<crate::cementation::ConfirmingSet>,
        aec_voter: AecVoter,
        aec_events: backpressure_channel::Receiver<AecFact>,
        lattice: UnsavedBlockLatticeBuilder,
        _tokio_runtime: tokio::runtime::Runtime,
        _test_dir: TestLedgerDir,
    }

    impl FullConfirmationFixture {
        fn new(batch_size: usize) -> Self {
            let test_dir = TestLedgerDir::new();
            let ledger = Arc::new(create_real_lmdb_ledger(&test_dir));
            let stats = Arc::new(Stats::default());
            let clock = Arc::new(SteadyClock::new_null());
            let queue = Arc::new(BlockProcessorQueue::new(ProcessQueueConfig {
                batch_size,
                ..Default::default()
            }));
            let aec = Arc::new(AecService::new(
                ActiveElectionsConfig {
                    max_elections: 20_000,
                    ..Default::default()
                },
                Duration::from_millis(25),
            ));
            let scheduler = PriorityScheduler::new(
                PriorityBucketConfig::default(),
                stats.clone(),
                aec.clone(),
                ledger.clone(),
                clock.clone(),
            );
            let (aec_tx, aec_events) = backpressure_channel::channel(1024 * 16);
            aec.set_observer(aec_tx.clone());

            let rep_key = PrivateKey::from(99_999);
            let rep_weights = ledger.rep_weights.clone();
            rep_weights.put(rep_key.public_key(), Amount::nano(80_000_000));
            let online_reps = Arc::new(Mutex::new(
                OnlineReps::builder()
                    .rep_weights(rep_weights.clone())
                    .online_weight_minimum(Amount::nano(60_000_000))
                    .representative_weight_minimum(Amount::nano(1000))
                    .finish(),
            ));
            let local_vote_history = Arc::new(LocalVoteHistory::new(NetworkType::NanoDevNetwork));
            let wallet_reps = Arc::new(Mutex::new(create_wallet_representatives(
                rep_key,
                rep_weights.clone(),
                online_reps.clone(),
            )));
            wallet_reps.lock().unwrap().compute_reps();
            assert!(wallet_reps.lock().unwrap().voting_enabled());

            let vote_processor_queue = Arc::new(VoteProcessorQueue::new(
                VoteProcessorConfig::new(1),
                stats.clone(),
            ));
            let vote_broadcaster = Arc::new(VoteBroadcaster::new(
                vote_processor_queue.clone(),
                MessageFlooder::new_null(),
                stats.clone(),
            ));
            let mut config = NodeConfig::new_test_instance();
            config.vote_generator_delay = Duration::from_millis(1);
            let network_params = NetworkParams::new(NetworkType::NanoDevNetwork);
            let vote_generators = Arc::new(VoteGenerators::new(
                ledger.clone(),
                wallet_reps,
                local_vote_history.clone(),
                stats.clone(),
                &config,
                &network_params,
                vote_broadcaster,
                MessageSender::new_null(),
                clock.clone(),
            ));
            let vote_processor = Arc::new(VoteProcessor::new(
                vote_processor_queue.clone(),
                VoteApplier::new(
                    aec.clone(),
                    online_reps.clone(),
                    clock.clone(),
                    rep_weights.clone(),
                    true,
                ),
                stats.clone(),
            ));
            vote_processor.add_observer(aec_tx);
            vote_generators.start();
            vote_processor.start();

            let confirming_set = Arc::new(crate::cementation::ConfirmingSet::new(
                crate::cementation::ConfirmingSetConfig {
                    batch_size,
                    ..Default::default()
                },
                ledger.clone(),
                stats.clone(),
            ));
            let aec_voter = AecVoter::new(
                aec.clone(),
                vote_generators.clone(),
                clock.clone(),
                NetworkType::NanoDevNetwork,
                CpsLimiter::unlimited(),
            );
            let vote_cache = Arc::new(Mutex::new(VoteCache::new(
                Default::default(),
                stats.clone(),
            )));
            let tokio_runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let aec_fact_processor = create_aec_fact_processor(
                ledger.clone(),
                queue.clone(),
                aec.clone(),
                confirming_set.clone(),
                vote_processor.clone(),
                vote_processor_queue.clone(),
                online_reps.clone(),
                rep_weights.clone(),
                vote_cache.clone(),
                local_vote_history.clone(),
                stats.clone(),
                clock.clone(),
                tokio_runtime.handle().clone(),
            );

            Self {
                ledger,
                queue,
                scheduler,
                aec,
                aec_fact_processor,
                vote_generators: Some(vote_generators),
                vote_processor: Some(vote_processor),
                confirming_set,
                aec_voter,
                aec_events,
                lattice: UnsavedBlockLatticeBuilder::with_stub_work(),
                _tokio_runtime: tokio_runtime,
                _test_dir: test_dir,
            }
        }

        fn process_and_confirm_live_blocks(&mut self, block_count: usize) -> FullPathTimingReport {
            for account_index in 0..block_count {
                push_live_open(
                    &self.ledger,
                    &self.queue,
                    &mut self.lattice,
                    account_index + 1,
                );
            }

            let dequeue_start = Instant::now();
            let batch = self.queue.pop_blocking().unwrap();
            let dequeue_elapsed = dequeue_start.elapsed();

            let process_start = Instant::now();
            let results = self
                .ledger
                .process_batch(batch.iter().map(|context| (&context.block, context.source)));
            let process_elapsed = process_start.elapsed();
            assert_eq!(results.len(), block_count);
            assert!(results.iter().all(|result| result.status.is_ok()));

            let saved_blocks = results
                .iter()
                .map(|result| result.saved_block.clone().unwrap())
                .collect::<Vec<_>>();

            let schedule_start = Instant::now();
            for block in &saved_blocks {
                self.scheduler
                    .activate(&self.ledger.any(), &block.account());
            }
            self.scheduler.run_one();
            let schedule_elapsed = schedule_start.elapsed();
            assert_eq!(self.aec.len(), block_count);

            let mut aec_fact_process_elapsed = Duration::ZERO;
            self.drain_and_process_aec_events(
                &mut Vec::new(),
                &mut Vec::new(),
                &mut Vec::new(),
                &mut aec_fact_process_elapsed,
            );

            let non_final_vote_start = Instant::now();
            self.aec_voter.tick(&CancellationToken::new_null());
            let mut non_final_vote_hash_counts = Vec::new();
            let mut non_final_vote_ok_counts = Vec::new();
            let non_final_vote_processed_events = self.wait_until_all_active_elections_are_final(
                block_count,
                &mut non_final_vote_hash_counts,
                &mut non_final_vote_ok_counts,
                &mut aec_fact_process_elapsed,
            );
            let non_final_vote_elapsed = non_final_vote_start.elapsed();

            let final_vote_start = Instant::now();
            self.aec_voter.tick(&CancellationToken::new_null());
            let mut confirmed_elections = Vec::new();
            let mut final_vote_hash_counts = Vec::new();
            let mut final_vote_ok_counts = Vec::new();
            let final_vote_processed_events = self.wait_until_elections_confirmed(
                block_count,
                &mut confirmed_elections,
                &mut final_vote_hash_counts,
                &mut final_vote_ok_counts,
                &mut aec_fact_process_elapsed,
            );
            let final_vote_elapsed = final_vote_start.elapsed();

            let confirmation_hashes = saved_blocks
                .iter()
                .map(|block| block.hash())
                .collect::<Vec<_>>();
            let cement_start = Instant::now();
            self.confirming_set.start();
            self.wait_until_blocks_cemented(&confirmation_hashes);
            let cement_elapsed = cement_start.elapsed();

            FullPathTimingReport {
                blocks: block_count,
                confirmed: confirmation_hashes.len(),
                generated_vote_batches: non_final_vote_processed_events
                    + final_vote_processed_events,
                non_final_vote_processed_events,
                final_vote_processed_events,
                non_final_vote_hash_counts,
                final_vote_hash_counts,
                non_final_vote_ok_counts,
                final_vote_ok_counts,
                dequeue_elapsed,
                process_elapsed,
                schedule_elapsed,
                non_final_vote_elapsed,
                final_vote_elapsed,
                aec_fact_process_elapsed,
                cement_elapsed,
            }
        }

        fn drain_and_process_aec_events(
            &mut self,
            confirmed_elections: &mut Vec<ConfirmedElection>,
            vote_hash_counts: &mut Vec<usize>,
            vote_ok_counts: &mut Vec<usize>,
            aec_fact_process_elapsed: &mut Duration,
        ) -> usize {
            let mut vote_processed_events = 0;
            loop {
                match self.aec_events.try_recv() {
                    Ok(event) => {
                        if let AecFact::ElectionConfirmed(election) = &event {
                            confirmed_elections.push(election.clone());
                        }
                        if let AecFact::VoteProcessed(vote, _, results) = &event {
                            vote_processed_events += 1;
                            vote_hash_counts.push(vote.vote.hashes.len());
                            vote_ok_counts
                                .push(results.values().filter(|result| result.is_ok()).count());
                        }
                        let process_start = Instant::now();
                        self.aec_fact_processor.process(event);
                        *aec_fact_process_elapsed += process_start.elapsed();
                    }
                    Err(TryRecvError::Empty) => return vote_processed_events,
                    Err(TryRecvError::Disconnected) => panic!("AEC event channel disconnected"),
                }
            }
        }

        fn wait_until_all_active_elections_are_final(
            &mut self,
            expected: usize,
            vote_hash_counts: &mut Vec<usize>,
            vote_ok_counts: &mut Vec<usize>,
            aec_fact_process_elapsed: &mut Duration,
        ) -> usize {
            let start = Instant::now();
            let mut vote_processed_events = 0;
            loop {
                vote_processed_events += self.drain_and_process_aec_events(
                    &mut Vec::new(),
                    vote_hash_counts,
                    vote_ok_counts,
                    aec_fact_process_elapsed,
                );
                let active_final = self.aec.round_robin(|elections| {
                    elections.filter(|election| election.is_final()).count()
                });
                if active_final == expected && vote_ok_counts.iter().sum::<usize>() == expected {
                    return vote_processed_events;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "timed out waiting for non-final votes to establish quorum: {active_final}/{expected}; vote_hash_counts={vote_hash_counts:?}; vote_ok_counts={vote_ok_counts:?}"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn wait_until_elections_confirmed(
            &mut self,
            expected: usize,
            confirmed_elections: &mut Vec<ConfirmedElection>,
            vote_hash_counts: &mut Vec<usize>,
            vote_ok_counts: &mut Vec<usize>,
            aec_fact_process_elapsed: &mut Duration,
        ) -> usize {
            let start = Instant::now();
            let mut vote_processed_events = 0;
            loop {
                vote_processed_events += self.drain_and_process_aec_events(
                    confirmed_elections,
                    vote_hash_counts,
                    vote_ok_counts,
                    aec_fact_process_elapsed,
                );
                if confirmed_elections.len() == expected {
                    return vote_processed_events;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "timed out waiting for final votes to confirm elections: {}/{expected}",
                    confirmed_elections.len()
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn wait_until_blocks_cemented(&self, confirmation_hashes: &[BlockHash]) {
            let start = Instant::now();
            loop {
                if confirmation_hashes
                    .iter()
                    .all(|hash| self.ledger.confirmed().block_exists(hash))
                    && self.confirming_set.is_empty()
                {
                    return;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "timed out waiting for confirming set to cement blocks"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    impl Drop for FullConfirmationFixture {
        fn drop(&mut self) {
            if let Some(vote_generators) = self.vote_generators.take() {
                vote_generators.stop();
            }
            if let Some(vote_processor) = self.vote_processor.take() {
                vote_processor.stop();
            }
        }
    }

    struct AecFactDispatchFixture {
        ledger: Arc<Ledger>,
        queue: Arc<BlockProcessorQueue>,
        confirming_set: Arc<crate::cementation::ConfirmingSet>,
        stats: Arc<Stats>,
        clock: Arc<SteadyClock>,
        aec: Arc<AecService>,
        vote_processor: Arc<VoteProcessor>,
        vote_processor_queue: Arc<VoteProcessorQueue>,
        online_reps: Arc<Mutex<OnlineReps>>,
        rep_weights: Arc<RepWeightCache>,
        vote_cache: Arc<Mutex<VoteCache>>,
        local_vote_history: Arc<LocalVoteHistory>,
        lattice: UnsavedBlockLatticeBuilder,
        _tokio_runtime: tokio::runtime::Runtime,
        _test_dir: TestLedgerDir,
    }

    impl AecFactDispatchFixture {
        fn new(batch_size: usize) -> Self {
            let test_dir = TestLedgerDir::new();
            let ledger = Arc::new(create_real_lmdb_ledger(&test_dir));
            let stats = Arc::new(Stats::default());
            let clock = Arc::new(SteadyClock::new_null());
            let queue = Arc::new(BlockProcessorQueue::new(ProcessQueueConfig {
                batch_size,
                ..Default::default()
            }));
            let aec = Arc::new(AecService::new(
                ActiveElectionsConfig {
                    max_elections: batch_size * prio_bucket_count(),
                    ..Default::default()
                },
                Duration::from_millis(25),
            ));
            let rep_weights = ledger.rep_weights.clone();
            let online_reps = Arc::new(Mutex::new(
                OnlineReps::builder()
                    .rep_weights(rep_weights.clone())
                    .online_weight_minimum(Amount::nano(60_000_000))
                    .representative_weight_minimum(Amount::nano(1000))
                    .finish(),
            ));
            let vote_cache = Arc::new(Mutex::new(VoteCache::new(
                Default::default(),
                stats.clone(),
            )));
            let vote_processor_queue = Arc::new(VoteProcessorQueue::new(
                VoteProcessorConfig::new(1),
                stats.clone(),
            ));
            let vote_processor = Arc::new(VoteProcessor::new(
                vote_processor_queue.clone(),
                VoteApplier::new(
                    aec.clone(),
                    online_reps.clone(),
                    clock.clone(),
                    rep_weights.clone(),
                    true,
                ),
                stats.clone(),
            ));
            let confirming_set = Arc::new(crate::cementation::ConfirmingSet::new(
                crate::cementation::ConfirmingSetConfig {
                    batch_size,
                    ..Default::default()
                },
                ledger.clone(),
                stats.clone(),
            ));
            let tokio_runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            Self {
                ledger,
                queue,
                confirming_set,
                stats,
                clock,
                aec,
                vote_processor,
                vote_processor_queue,
                online_reps,
                rep_weights,
                vote_cache,
                local_vote_history: Arc::new(LocalVoteHistory::new(NetworkType::NanoDevNetwork)),
                lattice: UnsavedBlockLatticeBuilder::with_stub_work(),
                _tokio_runtime: tokio_runtime,
                _test_dir: test_dir,
            }
        }

        fn dispatch_confirmed_elections_through_backpressure_thread(
            &mut self,
            block_count: usize,
            peer_count: usize,
        ) -> AecFactDispatchTimingReport {
            for account_index in 0..block_count {
                let peer_id = account_index % peer_count + 1;
                push_live_open_from_channel(
                    &self.ledger,
                    &self.queue,
                    &mut self.lattice,
                    account_index + 1,
                    ChannelId::from(peer_id),
                );
            }

            let dequeue_start = Instant::now();
            let batch = self.queue.pop_blocking().unwrap();
            let dequeue_elapsed = dequeue_start.elapsed();

            let process_start = Instant::now();
            let results = self
                .ledger
                .process_batch(batch.iter().map(|context| (&context.block, context.source)));
            let process_elapsed = process_start.elapsed();
            assert_eq!(results.len(), block_count);
            assert!(results.iter().all(|result| result.status.is_ok()));

            let saved_blocks = results
                .iter()
                .map(|result| result.saved_block.clone().unwrap())
                .collect::<Vec<_>>();
            let confirmation_hashes = saved_blocks
                .iter()
                .map(|block| block.hash())
                .collect::<Vec<_>>();
            let events = saved_blocks
                .into_iter()
                .map(|block| {
                    AecFact::ElectionConfirmed(ConfirmedElection::new(
                        block,
                        ConfirmationType::ActiveConfirmedQuorum,
                    ))
                })
                .collect::<Vec<_>>();
            let processor = create_aec_fact_processor(
                self.ledger.clone(),
                self.queue.clone(),
                self.aec.clone(),
                self.confirming_set.clone(),
                self.vote_processor.clone(),
                self.vote_processor_queue.clone(),
                self.online_reps.clone(),
                self.rep_weights.clone(),
                self.vote_cache.clone(),
                self.local_vote_history.clone(),
                self.stats.clone(),
                self.clock.clone(),
                self._tokio_runtime.handle().clone(),
            );
            let (tx, rx) = backpressure_channel::channel(block_count);
            spawn_backpressure_processor("AEC fact pressure", rx, processor);

            let dispatch_start = Instant::now();
            for event in events {
                tx.send(event).unwrap();
            }
            self.wait_until_confirming_set_len(block_count, Duration::from_secs(30));
            let aec_fact_thread_dispatch_elapsed = dispatch_start.elapsed();

            let cement_start = Instant::now();
            self.confirming_set.start();
            self.wait_until_blocks_cemented(&confirmation_hashes, Duration::from_secs(30));
            let cement_elapsed = cement_start.elapsed();

            AecFactDispatchTimingReport {
                blocks: block_count,
                peers: peer_count,
                confirmed: confirmation_hashes.len(),
                dequeue_elapsed,
                process_elapsed,
                aec_fact_thread_dispatch_elapsed,
                cement_elapsed,
            }
        }

        fn wait_until_confirming_set_len(&self, expected: usize, timeout: Duration) {
            let start = Instant::now();
            loop {
                if self.confirming_set.len() == expected {
                    return;
                }
                assert!(
                    start.elapsed() < timeout,
                    "timed out waiting for AEC facts to reach confirming set: {}/{expected}",
                    self.confirming_set.len()
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn wait_until_blocks_cemented(&self, confirmation_hashes: &[BlockHash], timeout: Duration) {
            let start = Instant::now();
            loop {
                if confirmation_hashes
                    .iter()
                    .all(|hash| self.ledger.confirmed().block_exists(hash))
                    && self.confirming_set.is_empty()
                {
                    return;
                }
                assert!(
                    start.elapsed() < timeout,
                    "timed out waiting for confirming set to cement blocks"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    struct FullPathTimingReport {
        blocks: usize,
        confirmed: usize,
        generated_vote_batches: usize,
        non_final_vote_processed_events: usize,
        final_vote_processed_events: usize,
        non_final_vote_hash_counts: Vec<usize>,
        final_vote_hash_counts: Vec<usize>,
        non_final_vote_ok_counts: Vec<usize>,
        final_vote_ok_counts: Vec<usize>,
        dequeue_elapsed: Duration,
        process_elapsed: Duration,
        schedule_elapsed: Duration,
        non_final_vote_elapsed: Duration,
        final_vote_elapsed: Duration,
        aec_fact_process_elapsed: Duration,
        cement_elapsed: Duration,
    }

    impl FullPathTimingReport {
        fn format(&self, label: &str) -> String {
            format!(
                "{label} blocks={} confirmed={} vote_batches={} vote_processed_events={} nonfinal_vote_hashes={:?} nonfinal_vote_ok={:?} final_vote_hashes={:?} final_vote_ok={:?} dequeue_us={} process_us={} schedule_us={} nonfinal_vote_to_quorum_us={} final_vote_to_election_confirmed_us={} aec_fact_process_us={} cement_us={} slowest_stage={}",
                self.blocks,
                self.confirmed,
                self.generated_vote_batches,
                self.non_final_vote_processed_events + self.final_vote_processed_events,
                self.non_final_vote_hash_counts,
                self.non_final_vote_ok_counts,
                self.final_vote_hash_counts,
                self.final_vote_ok_counts,
                self.dequeue_elapsed.as_micros(),
                self.process_elapsed.as_micros(),
                self.schedule_elapsed.as_micros(),
                self.non_final_vote_elapsed.as_micros(),
                self.final_vote_elapsed.as_micros(),
                self.aec_fact_process_elapsed.as_micros(),
                self.cement_elapsed.as_micros(),
                first_limiter([
                    ("dequeue", self.dequeue_elapsed),
                    ("process_batch", self.process_elapsed),
                    ("schedule", self.schedule_elapsed),
                    ("nonfinal_vote_to_quorum", self.non_final_vote_elapsed),
                    ("final_vote_to_election_confirmed", self.final_vote_elapsed),
                    ("aec_fact_process", self.aec_fact_process_elapsed),
                    ("cement", self.cement_elapsed),
                ])
            )
        }
    }

    struct AecFactDispatchTimingReport {
        blocks: usize,
        peers: usize,
        confirmed: usize,
        dequeue_elapsed: Duration,
        process_elapsed: Duration,
        aec_fact_thread_dispatch_elapsed: Duration,
        cement_elapsed: Duration,
    }

    impl AecFactDispatchTimingReport {
        fn format(&self, label: &str) -> String {
            format!(
                "{label} blocks={} peers={} confirmed={} dequeue_us={} process_us={} aec_fact_thread_dispatch_us={} cement_us={} slowest_stage={}",
                self.blocks,
                self.peers,
                self.confirmed,
                self.dequeue_elapsed.as_micros(),
                self.process_elapsed.as_micros(),
                self.aec_fact_thread_dispatch_elapsed.as_micros(),
                self.cement_elapsed.as_micros(),
                first_limiter([
                    ("dequeue", self.dequeue_elapsed),
                    ("process_batch", self.process_elapsed),
                    (
                        "aec_fact_thread_dispatch",
                        self.aec_fact_thread_dispatch_elapsed
                    ),
                    ("cement", self.cement_elapsed),
                ])
            )
        }
    }

    struct Fixture {
        ledger: Arc<Ledger>,
        scheduler: PriorityScheduler,
        aec: Arc<AecService>,
        lattice: Mutex<UnsavedBlockLatticeBuilder>,
    }

    impl Fixture {
        fn new(max_elections: usize, bucket_config: PriorityBucketConfig) -> Self {
            let ledger = Arc::new(Ledger::new_null());
            let aec = Arc::new(AecService::new(
                ActiveElectionsConfig {
                    max_elections,
                    ..Default::default()
                },
                Duration::from_secs(1),
            ));
            let scheduler = PriorityScheduler::new(
                bucket_config,
                Arc::new(Stats::default()),
                aec.clone(),
                ledger.clone(),
                Arc::new(SteadyClock::new_null()),
            );

            Self {
                ledger,
                scheduler,
                aec,
                lattice: Mutex::new(UnsavedBlockLatticeBuilder::with_stub_work()),
            }
        }

        fn process_open(&self, account: &PrivateKey, amount: impl Into<Amount>) -> Block {
            let mut lattice = self.lattice.lock().unwrap();
            let send = lattice.genesis().send(account, amount);
            let open = lattice.account(account).receive(&send);

            self.ledger.process_one(&send).unwrap();
            self.ledger.confirm(send.hash());
            self.ledger.process_one(&open).unwrap();

            open
        }

        fn process_open_without_confirmed_send(
            &self,
            account: &PrivateKey,
            amount: impl Into<Amount>,
        ) -> Block {
            let mut lattice = self.lattice.lock().unwrap();
            let send = lattice.genesis().send(account, amount);
            let open = lattice.account(account).receive(&send);

            self.ledger.process_one(&send).unwrap();
            self.ledger.process_one(&open).unwrap();

            open
        }

        fn activate(&self, account: rsnano_types::Account) {
            self.scheduler.activate(&self.ledger.any(), &account);
        }
    }

    fn create_aec_fact_processor(
        ledger: Arc<Ledger>,
        block_processor_queue: Arc<BlockProcessorQueue>,
        active_elections: Arc<AecService>,
        confirming_set: Arc<crate::cementation::ConfirmingSet>,
        vote_processor: Arc<VoteProcessor>,
        vote_processor_queue: Arc<VoteProcessorQueue>,
        online_reps: Arc<Mutex<OnlineReps>>,
        rep_weights: Arc<RepWeightCache>,
        vote_cache: Arc<Mutex<VoteCache>>,
        local_vote_history: Arc<LocalVoteHistory>,
        stats: Arc<Stats>,
        clock: Arc<SteadyClock>,
        tokio: tokio::runtime::Handle,
    ) -> AecFactProcessor {
        let network_params = NetworkParams::new(NetworkType::NanoDevNetwork);
        let node_config = NodeConfig::new_test_instance();
        let network = Arc::new(RwLock::new(Network::new_null()));
        let keepalive_factory = Arc::new(KeepaliveMessageFactory::new(
            network.clone(),
            Peer::new("::", 0),
        ));
        let keepalive_publisher = Arc::new(KeepalivePublisher::new(
            network.clone(),
            Arc::new(PeerConnector::new_null(tokio.clone())),
            MessageSender::new_null(),
            keepalive_factory,
        ));
        let rep_crawler = Arc::new(RepCrawler::new(
            online_reps.clone(),
            stats.clone(),
            node_config.rep_crawler_query_timeout,
            node_config,
            network_params,
            network,
            ledger.clone(),
            clock.clone(),
            MessageSender::new_null(),
            keepalive_publisher,
            active_elections.clone(),
            tokio,
        ));

        AecFactProcessor {
            vote_cache_processor: Arc::new(VoteCacheProcessor::new(
                stats.clone(),
                vote_cache.clone(),
                vote_processor_queue,
                VoteProcessorConfig::new(1),
            )),
            vote_processor,
            vote_cache: vote_cache.clone(),
            node_observer: None,
            election_schedulers: Arc::new(
                crate::consensus::election_schedulers::ElectionSchedulers::new_null(),
            ),
            network_filter: Arc::new(NetworkFilter::default()),
            bootstrap_election_activator: BootstrapElectionActivator {
                active_elections: active_elections.clone(),
                vote_cache: vote_cache.clone(),
                stats: stats.clone(),
            },
            recently_cemented_inserter: RecentlyCementedInserter {
                recently_cemented: Arc::new(Mutex::new(BoundedVecDeque::new(1024 * 64))),
            },
            vote_rebroadcast_queue: Arc::new(
                VoteRebroadcastQueue::build()
                    .stats(stats.clone())
                    .block_when_empty(false)
                    .finish(),
            ),
            block_processor_queue,
            confirming_set,
            online_reps,
            active_elections: active_elections.clone(),
            rep_crawler,
            clock,
            local_votes_remover: LocalVotesRemover {
                vote_history: local_vote_history,
                active_elections: active_elections.clone(),
            },
            stats,
            aec_fork_inserter: Arc::new(AecForkInserter {
                rep_weights,
                fork_cache: Arc::new(RwLock::new(crate::consensus::ForkCache::new())),
                active_elections,
                vote_cache,
            }),
            winner_block_broadcaster: Arc::new(Mutex::new(WinnerBlockBroadcaster::new_null())),
            bootstrapper: Arc::new(Bootstrapper::new_null()),
        }
    }

    fn create_real_lmdb_ledger(test_dir: &TestLedgerDir) -> Ledger {
        LedgerBuilder::new(test_dir.ledger_path())
            .config(LmdbConfig {
                sync: SyncStrategy::NosyncUnsafe,
                map_size: 128 * 1024 * 1024,
                ..Default::default()
            })
            .constants(rsnano_ledger::LedgerConstants::unit_test())
            .init_thread_count(1)
            .consistency_check(false)
            .finish()
            .unwrap()
    }

    fn push_live_open(
        ledger: &Ledger,
        queue: &BlockProcessorQueue,
        lattice: &mut UnsavedBlockLatticeBuilder,
        account_index: usize,
    ) {
        push_live_open_from_channel(ledger, queue, lattice, account_index, ChannelId::LOOPBACK);
    }

    fn push_live_open_from_channel(
        ledger: &Ledger,
        queue: &BlockProcessorQueue,
        lattice: &mut UnsavedBlockLatticeBuilder,
        account_index: usize,
        channel_id: ChannelId,
    ) {
        let account = PrivateKey::from(account_index as u64);
        let send = lattice.genesis().send(&account, 1);
        let open = lattice.account(&account).receive(&send);

        ledger.process_one(&send).unwrap();
        ledger.confirm(send.hash());
        assert!(queue.push(BlockContext::new(open, BlockSource::Live, channel_id,)));
    }

    fn create_wallet_representatives(
        rep_key: PrivateKey,
        rep_weights: Arc<RepWeightCache>,
        online_reps: Arc<Mutex<OnlineReps>>,
    ) -> WalletRepresentatives {
        let wallets = Arc::new(Wallets::new_null());
        let wallet_id = WalletId::random();
        wallets.create(wallet_id);
        wallets
            .insert_adhoc2(&wallet_id, &rep_key.raw_key(), false)
            .unwrap();
        WalletRepresentatives::new(true, Amount::nano(1000), rep_weights, wallets, online_reps)
    }

    fn first_limiter<const N: usize>(stages: [(&'static str, Duration); N]) -> &'static str {
        stages
            .into_iter()
            .max_by_key(|(_, elapsed)| elapsed.as_nanos())
            .map(|(stage, _)| stage)
            .unwrap()
    }

    #[derive(Default)]
    struct TimingCementingObserver {
        already_confirmed: Vec<rsnano_types::BlockHash>,
        failed: Vec<rsnano_types::BlockHash>,
    }

    impl CementingObserver for TimingCementingObserver {
        fn already_confirmed(&mut self, hash: &rsnano_types::BlockHash) {
            self.already_confirmed.push(*hash);
        }

        fn cementing_failed(&mut self, hash: &rsnano_types::BlockHash) {
            self.failed.push(*hash);
        }
    }

    struct TestLedgerDir {
        path: PathBuf,
    }

    impl TestLedgerDir {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let id = TEST_LEDGER_DIR_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "rsnano-live-pipeline-pressure-{}-{unique}-{id}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn ledger_path(&self) -> PathBuf {
            self.path.join("data.ldb")
        }
    }

    impl Drop for TestLedgerDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
