use std::{
    net::SocketAddrV6,
    sync::{Arc, Mutex, RwLock},
};

use tracing::trace;

use rsnano_messages::{Message, NetworkFilter};
use rsnano_network::{Channel, Network};
use rsnano_types::VoteSource;
use rsnano_utils::stats::{DetailType, Direction, StatType, Stats};
use rsnano_work::WorkThresholds;

#[cfg(feature = "ledger_snapshots")]
use crate::ledger_snapshots::LedgerSnapshots;
use crate::{
    block_processing::{BlockContext, BlockProcessorQueue},
    bootstrap::{BootstrapServer, Bootstrapper},
    consensus::{AggregatorRequest, RequestAggregator, VoteProcessorQueue},
    telemetry::Telemetry,
    wallets::WalletRepresentatives,
};
use rsnano_ledger::BlockSource;

/// Process messages that were received from other nodes in the network
pub struct NetworkMessageProcessor {
    stats: Arc<Stats>,
    network_filter: Arc<NetworkFilter>,
    network: Arc<RwLock<Network>>,
    block_processor_queue: Arc<BlockProcessorQueue>,
    wallet_reps: Arc<Mutex<WalletRepresentatives>>,
    request_aggregator: Arc<RequestAggregator>,
    vote_processor_queue: Arc<VoteProcessorQueue>,
    telemetry: Arc<Telemetry>,
    bootstrap_server: Arc<BootstrapServer>,
    bootstrapper: Arc<Bootstrapper>,
    work_thresholds: WorkThresholds,
    #[cfg(feature = "ledger_snapshots")]
    ledger_snapshots: Arc<LedgerSnapshots>,
}

impl NetworkMessageProcessor {
    pub(crate) fn new(
        stats: Arc<Stats>,
        network: Arc<RwLock<Network>>,
        network_filter: Arc<NetworkFilter>,
        block_processor_queue: Arc<BlockProcessorQueue>,
        wallet_reps: Arc<Mutex<WalletRepresentatives>>,
        request_aggregator: Arc<RequestAggregator>,
        vote_processor_queue: Arc<VoteProcessorQueue>,
        telemetry: Arc<Telemetry>,
        bootstrap_server: Arc<BootstrapServer>,
        bootstrapper: Arc<Bootstrapper>,
        work_thresholds: WorkThresholds,
        #[cfg(feature = "ledger_snapshots")] ledger_snapshots: Arc<LedgerSnapshots>,
    ) -> Self {
        Self {
            stats,
            network,
            network_filter,
            block_processor_queue,
            wallet_reps,
            request_aggregator,
            vote_processor_queue,
            telemetry,
            bootstrap_server,
            bootstrapper,
            work_thresholds,
            #[cfg(feature = "ledger_snapshots")]
            ledger_snapshots,
        }
    }

    pub fn process(&self, message: Message, channel: &Arc<Channel>) {
        self.stats.inc_dir(
            StatType::Message,
            message.message_type().into(),
            Direction::In,
        );

        trace!(
            ?message,
            channel_id = ?channel.channel_id(),
            "network processed"
        );

        match message {
            Message::Keepalive(keepalive) => {
                // Check for special node port data
                let peer0 = keepalive.peers[0];
                // The first entry is used to inform us of the peering address of the sending node
                if peer0.ip().is_unspecified() && peer0.port() != 0 {
                    let peering_addr =
                        SocketAddrV6::new(*channel.peer_addr().ip(), peer0.port(), 0, 0);

                    // Remember this for future forwarding to other peers
                    self.network
                        .read()
                        .unwrap()
                        .set_peering_addr(channel.channel_id(), peering_addr);
                }
            }
            Message::Publish(publish) => {
                let mut ok = true;

                if !self.work_thresholds.validate_entry_block(&publish.block) {
                    self.stats
                        .inc(StatType::BlockProcessor, DetailType::InsufficientWork);
                    ok = false;
                }

                if ok {
                    // Put blocks that are being initially broadcasted in a separate queue, so that they won't have to compete with rebroadcasted blocks
                    // Both queues have the same priority and size, so the potential for exploiting this is limited
                    let source = if publish.is_originator {
                        BlockSource::LiveOriginator
                    } else {
                        BlockSource::Live
                    };

                    trace!(block_hash = ?publish.block.hash(), channel_id = ?channel.channel_id(), "Received publish");

                    ok = self.block_processor_queue.push(BlockContext::new(
                        publish.block,
                        source,
                        channel.channel_id(),
                    ));
                }

                if !ok {
                    // The message couldn't be handled. We have to remove it from the duplicate
                    // filter, so that it can be retransmitted and handled later
                    self.network_filter.clear(publish.digest);
                    self.stats
                        .inc_dir(StatType::Drop, DetailType::Publish, Direction::In);
                }
            }
            Message::ConfirmReq(req) => {
                // Don't load nodes with disabled voting
                // TODO: This check should be cached somewhere
                if self.wallet_reps.lock().unwrap().voting_enabled() {
                    let aggregator_req = AggregatorRequest {
                        channel: channel.clone(),
                        roots_hashes: req.roots_hashes,
                    };
                    self.request_aggregator.request(aggregator_req);
                }
            }
            Message::ConfirmAck(ack) => {
                // Ignore zero account votes
                if ack.vote().voter.is_zero() {
                    self.stats.inc_dir(
                        StatType::Drop,
                        DetailType::ConfirmAckZeroAccount,
                        Direction::In,
                    );
                }

                let source = match ack.is_rebroadcasted() {
                    true => VoteSource::Rebroadcast,
                    false => VoteSource::Live,
                };

                let added = self.vote_processor_queue.enqueue(
                    Arc::new(ack.vote().clone()),
                    Some(channel.clone()),
                    source,
                    None,
                );

                if !added {
                    // The message couldn't be handled. We have to remove it from the duplicate
                    // filter, so that it can be retransmitted and handled later
                    self.network_filter.clear(ack.digest);
                    self.stats
                        .inc_dir(StatType::Drop, DetailType::ConfirmAck, Direction::In);
                }
            }
            Message::Handshake(_) => {
                self.stats.inc_dir(
                    StatType::Message,
                    DetailType::NodeIdHandshake,
                    Direction::In,
                );
            }
            Message::TelemetryReq => {
                // Ignore telemetry requests as telemetry is being periodically broadcasted since V25+
            }
            Message::TelemetryAck(ack) => self.telemetry.process(&ack, channel),
            Message::AscPullReq(req) => {
                self.bootstrap_server.enqueue(req, channel.clone());
            }
            Message::AscPullAck(ack) => self.bootstrapper.process(ack, channel.channel_id()),
            Message::FrontierReq(_)
            | Message::BulkPush
            | Message::BulkPull(_)
            | Message::BulkPullAccount(_) => {
                // obsolete messages
            }
            #[cfg(feature = "ledger_snapshots")]
            Message::SnapshotPreproposal(preproposal) => {
                self.ledger_snapshots.handle_preproposal(preproposal);
            }
            #[cfg(feature = "ledger_snapshots")]
            Message::SnapshotProposal(proposal) => {
                self.ledger_snapshots.handle_proposal(proposal);
            }
            #[cfg(feature = "ledger_snapshots")]
            Message::SnapshotProposalVote(proposal_vote) => {
                self.ledger_snapshots.handle_vote(proposal_vote);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        consensus::{
            AecFact, AecInsertRequest, AecService, VoteApplier, VoteProcessor,
            VoteProcessorConfig, VoteProcessorExt,
        },
        representatives::{OnlineReps, VoteQuorumPreparer},
    };
    use rsnano_ledger::RepWeightCache;
    use rsnano_messages::ConfirmAck;
    use rsnano_nullable_clock::{SteadyClock, Timestamp};
    use rsnano_types::{Amount, BlockPriority, PrivateKey, SavedBlock, Vote};
    use rsnano_utils::{
        stats::Stats,
        sync::backpressure_channel::{Receiver, channel},
    };
    use std::{
        sync::{Arc, Condvar, Mutex},
        time::Duration,
    };

    #[test]
    fn confirm_ack_for_other_election_completes_while_first_vote_is_blocked_in_ingress_path() {
        let first_rep = PrivateKey::from(1);
        let second_rep = PrivateKey::from(2);

        let rep_weights = Arc::new(RepWeightCache::default());
        rep_weights.put(first_rep.public_key(), Amount::nano(80_000_000));
        rep_weights.put(second_rep.public_key(), Amount::nano(90_000_000));

        let online_reps = Arc::new(Mutex::new(
            OnlineReps::builder()
                .rep_weights(rep_weights.clone())
                .finish(),
        ));
        let entered = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let blocked_voter = first_rep.public_key();
        let quorum_preparer = Arc::new(VoteQuorumPreparer::new_with_hook(
            online_reps,
            Arc::new({
                let entered = entered.clone();
                let release = release.clone();
                move |voter| {
                    if voter != blocked_voter {
                        return;
                    }

                    let (entered_lock, entered_condition) = &*entered;
                    *entered_lock.lock().unwrap() = true;
                    entered_condition.notify_all();

                    let (release_lock, release_condition) = &*release;
                    let _guard = release_condition
                        .wait_timeout_while(
                            release_lock.lock().unwrap(),
                            Duration::from_secs(1),
                            |released| !*released,
                        )
                        .unwrap()
                        .0;
                }
            }),
        ));

        let aec = Arc::new(AecService::new_null());
        let block_a = SavedBlock::new_test_instance_with_key(11);
        let block_b = SavedBlock::new_test_instance_with_key(22);
        let now = Timestamp::new_test_instance();

        aec.insert(
            AecInsertRequest::new_priority(block_a.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();
        aec.insert(
            AecInsertRequest::new_priority(block_b.clone(), BlockPriority::new_test_instance()),
            now,
        )
        .unwrap();

        let vote_applier = VoteApplier::new(
            aec.clone(),
            quorum_preparer,
            Arc::new(SteadyClock::new_null()),
            rep_weights,
            false,
        );

        let mut config = VoteProcessorConfig::new(4);
        config.threads = 2;
        config.batch_size = 2;
        let queue = Arc::new(VoteProcessorQueue::new(config, Arc::new(Stats::default())));
        let processor = Arc::new(VoteProcessor::new(
            queue.clone(),
            vote_applier,
            Arc::new(Stats::default()),
        ));
        let (tx, rx) = channel(16);
        processor.add_observer(tx);
        processor.start();

        let processor_guard = ProcessorGuard {
            processor: processor.clone(),
        };

        let network_message_processor = create_network_message_processor(queue.clone());
        let channel = Arc::new(Channel::new_test_instance());

        network_message_processor.process(
            Message::ConfirmAck(ConfirmAck::new_with_own_vote(Vote::new_final(
                &first_rep,
                vec![block_a.hash()],
            ))),
            &channel,
        );

        wait_until_blocked(&entered, Duration::from_millis(200));

        network_message_processor.process(
            Message::ConfirmAck(ConfirmAck::new_with_own_vote(Vote::new_final(
                &second_rep,
                vec![block_b.hash()],
            ))),
            &channel,
        );

        wait_for(
            Duration::from_secs(1),
            || aec.was_recently_confirmed(&block_b.hash()),
            "second confirm_ack should complete while the first remains blocked in the ingress path",
        );

        assert!(received_vote_processed(&rx, second_rep.public_key(), block_b.hash()));

        release_blocker(&release);
        drop(processor_guard);
    }

    fn wait_until_blocked(entered: &Arc<(Mutex<bool>, Condvar)>, timeout: Duration) {
        let (lock, condition) = &**entered;
        let blocked = condition
            .wait_timeout_while(lock.lock().unwrap(), timeout, |blocked| !*blocked)
            .unwrap()
            .0;
        assert!(*blocked, "timed out waiting for blocked confirm_ack path");
    }

    fn release_blocker(release: &Arc<(Mutex<bool>, Condvar)>) {
        let (lock, condition) = &**release;
        *lock.lock().unwrap() = true;
        condition.notify_all();
    }

    fn received_vote_processed(
        rx: &Receiver<AecFact>,
        voter: rsnano_types::PublicKey,
        block_hash: rsnano_types::BlockHash,
    ) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(1) {
            if let Ok(AecFact::VoteProcessed(vote, _, results)) = rx.try_recv()
                && vote.vote.voter == voter
                && results.get(&block_hash) == Some(&Ok(()))
            {
                return true;
            }

            std::thread::yield_now();
        }

        false
    }

    fn wait_for(timeout: Duration, mut predicate: impl FnMut() -> bool, message: &str) {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if predicate() {
                return;
            }

            std::thread::yield_now();
        }

        panic!("{message}");
    }

    struct ProcessorGuard {
        processor: Arc<VoteProcessor>,
    }

    impl Drop for ProcessorGuard {
        fn drop(&mut self) {
            self.processor.stop();
        }
    }

    #[cfg(feature = "ledger_snapshots")]
    #[test]
    fn preproposal_is_received() {
        use rsnano_messages::Preproposal;

        let ledger_snapshots = LedgerSnapshots::new_null();
        let receive_tracker = ledger_snapshots.track_received_preproposals();
        let network_message_processor = create_network_message_processor_with_snapshots(ledger_snapshots);
        let preproposal = Preproposal::new_test_instance();

        network_message_processor.process(
            Message::SnapshotPreproposal(preproposal.clone()),
            &Channel::new_test_instance().into(),
        );

        assert_eq!(receive_tracker.output(), vec![preproposal]);
    }

    #[cfg(feature = "ledger_snapshots")]
    #[test]
    fn proposal_is_received() {
        use rsnano_messages::Proposal;

        let ledger_snapshots: LedgerSnapshots = LedgerSnapshots::new_null();
        let receive_tracker = ledger_snapshots.track_received_proposals();
        let network_message_processor = create_network_message_processor_with_snapshots(ledger_snapshots);
        let proposal = Proposal::new_test_instance();

        network_message_processor.process(
            Message::SnapshotProposal(proposal.clone()),
            &Channel::new_test_instance().into(),
        );

        assert_eq!(receive_tracker.output(), vec![proposal]);
    }

    #[cfg(feature = "ledger_snapshots")]
    #[test]
    fn proposal_vote_is_received() {
        use rsnano_messages::ProposalVote;

        let ledger_snapshots: LedgerSnapshots = LedgerSnapshots::new_null();
        let receive_tracker = ledger_snapshots.track_received_votes();
        let network_message_processor = create_network_message_processor_with_snapshots(ledger_snapshots);
        let proposal_vote = ProposalVote::new_test_instance();

        network_message_processor.process(
            Message::SnapshotProposalVote(proposal_vote.clone()),
            &Channel::new_test_instance().into(),
        );

        assert_eq!(receive_tracker.output(), vec![proposal_vote]);
    }

    fn create_network_message_processor(
        vote_processor_queue: Arc<VoteProcessorQueue>,
    ) -> NetworkMessageProcessor {
        NetworkMessageProcessor::new(
            Stats::default().into(),
            RwLock::new(Network::new_test_instance()).into(),
            NetworkFilter::default().into(),
            BlockProcessorQueue::new_null().into(),
            Mutex::new(WalletRepresentatives::new_null()).into(),
            RequestAggregator::new_null().into(),
            vote_processor_queue,
            Telemetry::new_null().into(),
            BootstrapServer::new_null().into(),
            Bootstrapper::new_null().into(),
            WorkThresholds::new_stub(),
            #[cfg(feature = "ledger_snapshots")]
            LedgerSnapshots::new_null().into(),
        )
    }

    #[cfg(feature = "ledger_snapshots")]
    fn create_network_message_processor_with_snapshots(
        ledger_snapshots: LedgerSnapshots,
    ) -> NetworkMessageProcessor {
        NetworkMessageProcessor::new(
            Stats::default().into(),
            RwLock::new(Network::new_test_instance()).into(),
            NetworkFilter::default().into(),
            BlockProcessorQueue::new_null().into(),
            Mutex::new(WalletRepresentatives::new_null()).into(),
            RequestAggregator::new_null().into(),
            VoteProcessorQueue::new_null().into(),
            Telemetry::new_null().into(),
            BootstrapServer::new_null().into(),
            Bootstrapper::new_null().into(),
            WorkThresholds::new_stub(),
            ledger_snapshots.into(),
        )
    }
}
