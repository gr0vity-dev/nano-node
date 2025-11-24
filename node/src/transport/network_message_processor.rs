use std::{
    net::SocketAddrV6,
    sync::{Arc, Mutex, RwLock},
};

use tracing::{trace, warn};

use rsnano_messages::{Message, NetworkFilter};
use rsnano_network::{Channel, Network};
use rsnano_types::VoteSource;
use rsnano_utils::stats::{DetailType, Direction, StatType, Stats};

#[cfg(feature = "ledger_snapshots")]
use crate::ledger_snapshots::LedgerSnapshots;
use crate::{
    block_processing::BlockSource,
    bootstrap::{BootstrapServer, Bootstrapper},
    consensus::{AggregatorRequest, RequestAggregator, VoteProcessorQueue},
    services::block_submitter::BlockSubmitter,
    telemetry::Telemetry,
    wallets::WalletRepresentatives,
};

/// Process messages that were received from other nodes in the network
pub struct NetworkMessageProcessor {
    stats: Arc<Stats>,
    network_filter: Arc<NetworkFilter>,
    network: Arc<RwLock<Network>>,
    block_submitter: Arc<BlockSubmitter>,
    wallet_reps: Arc<Mutex<WalletRepresentatives>>,
    request_aggregator: Arc<RequestAggregator>,
    vote_processor_queue: Arc<VoteProcessorQueue>,
    telemetry: Arc<Telemetry>,
    bootstrap_server: Arc<BootstrapServer>,
    bootstrapper: Arc<Bootstrapper>,
    #[cfg(feature = "ledger_snapshots")]
    ledger_snapshots: Arc<LedgerSnapshots>,
}

impl NetworkMessageProcessor {
    pub(crate) fn new(
        stats: Arc<Stats>,
        network: Arc<RwLock<Network>>,
        network_filter: Arc<NetworkFilter>,
        block_submitter: Arc<BlockSubmitter>,
        wallet_reps: Arc<Mutex<WalletRepresentatives>>,
        request_aggregator: Arc<RequestAggregator>,
        vote_processor_queue: Arc<VoteProcessorQueue>,
        telemetry: Arc<Telemetry>,
        bootstrap_server: Arc<BootstrapServer>,
        bootstrapper: Arc<Bootstrapper>,
        #[cfg(feature = "ledger_snapshots")] ledger_snapshots: Arc<LedgerSnapshots>,
    ) -> Self {
        Self {
            stats,
            network,
            network_filter,
            block_submitter,
            wallet_reps,
            request_aggregator,
            vote_processor_queue,
            telemetry,
            bootstrap_server,
            bootstrapper,
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
                let digest = publish.digest;

                // Put blocks that are being initially broadcasted in a separate queue, so that they won't have to compete with rebroadcasted blocks
                // Both queues have the same priority and size, so the potential for exploiting this is limited
                let source = if publish.is_originator {
                    BlockSource::LiveOriginator
                } else {
                    BlockSource::Live
                };

                let block_hash = publish.block.hash();
                trace!(block_hash = ?block_hash, channel_id = ?channel.channel_id(), "Received publish");

                let result = match source {
                    BlockSource::LiveOriginator => self
                        .block_submitter
                        .submit_live_originator(publish.block, channel.channel_id()),
                    _ => self
                        .block_submitter
                        .submit_live(publish.block, channel.channel_id()),
                };

                if let Err(err) = result {
                    // The message couldn't be handled. We have to remove it from the duplicate
                    // filter, so that it can be retransmitted and handled later
                    self.network_filter.clear(digest);
                    self.stats
                        .inc_dir(StatType::Drop, DetailType::Publish, Direction::In);

                    warn!(block_hash = ?block_hash, channel_id = ?channel.channel_id(), ?err, "failed to submit publish for processing");
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
            Message::NodeIdHandshake(_) => {
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

#[cfg(feature = "ledger_snapshots")]
#[cfg(test)]
mod tests {
    use super::*;
    // use std::sync::{Arc, Mutex, RwLock, atomic::AtomicBool};

    // use crate::{
    //     block_processing::ProcessQueueConfig,
    //     services::block_submitter::BlockSubmitter,
    // };
    // use rsnano_work::WorkThresholds;

    #[test]
    fn preproposal_is_received() {
        use rsnano_messages::Preproposal;

        let ledger_snapshots = LedgerSnapshots::new_null();
        let receive_tracker = ledger_snapshots.track_received_preproposals();
        let network_message_processor = create_network_message_processor(ledger_snapshots);
        let preproposal = Preproposal::new_test_instance();

        network_message_processor.process(
            Message::SnapshotPreproposal(preproposal.clone()),
            &Channel::new_test_instance().into(),
        );

        assert_eq!(receive_tracker.output(), vec![preproposal]);
    }

    #[test]
    fn proposal_is_received() {
        use rsnano_messages::Proposal;

        let ledger_snapshots: LedgerSnapshots = LedgerSnapshots::new_null();
        let receive_tracker = ledger_snapshots.track_received_proposals();
        let network_message_processor = create_network_message_processor(ledger_snapshots);
        let proposal = Proposal::new_test_instance();

        network_message_processor.process(
            Message::SnapshotProposal(proposal.clone()),
            &Channel::new_test_instance().into(),
        );

        assert_eq!(receive_tracker.output(), vec![proposal]);
    }

    #[test]
    fn proposal_vote_is_received() {
        use rsnano_messages::ProposalVote;

        let ledger_snapshots: LedgerSnapshots = LedgerSnapshots::new_null();
        let receive_tracker = ledger_snapshots.track_received_votes();
        let network_message_processor = create_network_message_processor(ledger_snapshots);
        let proposal_vote = ProposalVote::new_test_instance();

        network_message_processor.process(
            Message::SnapshotProposalVote(proposal_vote.clone()),
            &Channel::new_test_instance().into(),
        );

        assert_eq!(receive_tracker.output(), vec![proposal_vote]);
    }

    fn create_network_message_processor(
        ledger_snapshots: LedgerSnapshots,
    ) -> NetworkMessageProcessor {
        NetworkMessageProcessor::new(
            Arc::new(Stats::default()),
            Arc::new(RwLock::new(Network::new_test_instance())),
            Arc::new(NetworkFilter::default()),
            Arc::new(BlockSubmitter::new_null()),
            Arc::new(Mutex::new(WalletRepresentatives::new_null())),
            Arc::new(RequestAggregator::new_null()),
            Arc::new(VoteProcessorQueue::new_null()),
            Arc::new(Telemetry::new_null()),
            Arc::new(BootstrapServer::new_null()),
            Arc::new(Bootstrapper::new_null()),
            ledger_snapshots.into(),
        )
    }
}
