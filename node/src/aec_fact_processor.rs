use std::sync::{Arc, Mutex, mpsc::SyncSender};

use tracing::debug;

use rsnano_ledger::BlockSource;
use rsnano_messages::NetworkFilter;
use rsnano_network::ChannelId;
use rsnano_nullable_clock::SteadyClock;
use rsnano_types::{Block, VoteSource};
use rsnano_utils::stats::{Sample, Stats};

use crate::{
    NodeEvent,
    block_processing::{BlockContext, BlockProcessorQueue},
    cementation::ConfirmingSet,
    consensus::{
        AecCooldownReason, AecFact, AecForkInserter, AecService, BootstrapElectionActivator,
        LocalVotesRemover, ReceivedVote, VoteCache, VoteCacheProcessor, VoteProcessor,
        VoteRebroadcastQueue, WinnerBlockBroadcaster, aggregate_vote_results,
        election_schedulers::ElectionSchedulers,
    },
    recently_cemented_inserter::RecentlyCementedInserter,
    representatives::RepCrawler,
    utils::BackpressureEventProcessor,
};

/// Processes facts from the active election container (AEC)
pub(crate) struct AecFactProcessor {
    pub(crate) vote_cache_processor: Arc<VoteCacheProcessor>,
    pub(crate) vote_processor: Arc<VoteProcessor>,
    pub(crate) vote_cache: Arc<Mutex<VoteCache>>,
    pub(crate) node_observer: Option<SyncSender<NodeEvent>>,
    pub(crate) election_schedulers: Arc<ElectionSchedulers>,
    pub(crate) network_filter: Arc<NetworkFilter>,
    pub(crate) bootstrap_election_activator: BootstrapElectionActivator,
    pub(crate) recently_cemented_inserter: RecentlyCementedInserter,
    pub(crate) vote_rebroadcast_queue: Arc<VoteRebroadcastQueue>,
    pub(crate) block_processor_queue: Arc<BlockProcessorQueue>,
    pub(crate) confirming_set: Arc<ConfirmingSet>,
    pub(crate) active_elections: Arc<AecService>,
    pub(crate) rep_crawler: Arc<RepCrawler>,
    pub(crate) clock: Arc<SteadyClock>,
    pub(crate) local_votes_remover: LocalVotesRemover,
    pub(crate) stats: Arc<Stats>,
    pub(crate) aec_fork_inserter: Arc<AecForkInserter>,
    pub(crate) winner_block_broadcaster: Arc<Mutex<WinnerBlockBroadcaster>>,
}

impl BackpressureEventProcessor<AecFact> for AecFactProcessor {
    fn cool_down(&mut self) {
        self.active_elections
            .set_cooldown(true, AecCooldownReason::AecFactQueueFull);
        self.vote_processor.cool_down();
    }

    fn recovered(&mut self) {
        self.active_elections
            .set_cooldown(false, AecCooldownReason::AecFactQueueFull);
        self.vote_processor.recovered();
    }

    fn process(&mut self, event: AecFact) {
        match event {
            AecFact::ElectionStarted(hash, root) => {
                self.aec_fork_inserter.try_add_cached_forks(&root);
                self.bootstrap_election_activator.election_started(hash);
                self.vote_cache_processor.trigger(hash);
                if let Some(tx) = &self.node_observer {
                    tx.send(NodeEvent::ElectionStarted(hash)).unwrap();
                }
            }
            AecFact::ElectionConfirmed(election) => {
                self.confirming_set.add(election.clone());
                // Ensure election winner is broadcasted
                self.winner_block_broadcaster
                    .lock()
                    .unwrap()
                    .try_broadcast_winner(&election.winner, &election.votes);
            }
            AecFact::ElectionEnded(election) => {
                self.election_schedulers.notify();

                let now = self.clock.now();
                let elapsed = election.start().elapsed(now);
                // Track election duration
                self.stats.sample(
                    Sample::ActiveElectionDuration,
                    elapsed.as_millis() as i64,
                    (0, 1000 * 60 * 10),
                ); // 0-10 minutes range

                for (hash, block) in election.candidate_blocks() {
                    // Notify observers about dropped elections & blocks lost confirmed elections
                    if (!election.is_confirmed() || *hash != election.winner().hash())
                        && let Some(tx) = &self.node_observer
                    {
                        tx.send(NodeEvent::ElectionStopped(*hash)).unwrap();
                    }

                    if !election.is_confirmed() {
                        self.clear_network_filter(block);
                    }
                }
            }
            AecFact::BlockAddedToElection(hash) => self.vote_cache_processor.trigger(hash),
            AecFact::BlockDiscarded(block) => {
                self.clear_network_filter(&block);
            }
            AecFact::WinnerChanged(previous_winner, new_winner) => {
                debug!(from = ?previous_winner, to = ?new_winner.hash(), "Winning fork changed");
                self.local_votes_remover
                    .remove_local_votes(&previous_winner, &new_winner.qualified_root());

                // Roll back the previous winner and add the new winner to the ledger
                self.block_processor_queue.push(BlockContext::new(
                    new_winner.clone(),
                    BlockSource::Forced,
                    ChannelId::LOOPBACK,
                ));
            }
            AecFact::VoteProcessed(vote, voter_weight, results) => {
                // Cache the votes that didn't match any election
                if vote.source != VoteSource::Cache {
                    self.vote_cache
                        .lock()
                        .unwrap()
                        .insert(&vote.vote, voter_weight, &results);
                }

                self.vote_rebroadcast_queue
                    .try_enqueue(&vote.vote, &results);

                let result = aggregate_vote_results(&results);
                Self::process_vote_observation_follow_up(&self.rep_crawler, &vote);

                if let Some(tx) = &self.node_observer {
                    tx.send(NodeEvent::VoteProcessed(vote.vote, result))
                        .unwrap();
                }
            }
            AecFact::BlockConfirmed(block, election) => {
                if let Some(tx) = &self.node_observer {
                    tx.send(NodeEvent::BlockConfirmed(block, election.clone()))
                        .unwrap();
                }
                self.recently_cemented_inserter.insert(election);
            }
            AecFact::Recovered => self.election_schedulers.notify(),
        }
    }
}

impl AecFactProcessor {
    fn clear_network_filter(&mut self, block: &Block) {
        let mut buffer = Vec::new();
        block
            .serialize_without_block_type(&mut buffer)
            .expect("Should serialize block successfully");
        self.network_filter.clear_bytes(&buffer);
    }

    fn process_vote_observation_follow_up(rep_crawler: &RepCrawler, vote: &ReceivedVote) {
        // Ignore republished votes when rep crawling
        if vote.source == VoteSource::Live {
            rep_crawler.process(vote);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{NetworkParams, NodeConfig},
        representatives::{OnlineReps, VoteQuorumPreparer},
        transport::{
            MessageSender,
            keepalive::{KeepaliveMessageFactory, KeepalivePublisher},
        },
    };
    use rsnano_ledger::{Ledger, RepWeightCache};
    use rsnano_network::{Network, PeerConnector};
    use rsnano_nullable_clock::Timestamp;
    use rsnano_types::{
        Amount, BlockHash, Peer, PrivateKey, UnixMillisTimestamp, Vote, VoteSource,
    };
    use rsnano_utils::stats::Stats;
    use std::{sync::RwLock, time::Duration};

    #[test]
    fn vote_processed_follow_up_does_not_refresh_hot_path_observation() {
        let rep = PrivateKey::from(1);
        let rep_weights = Arc::new(RepWeightCache::default());
        rep_weights.put(rep.public_key(), Amount::nano(80_000_000));

        let online_reps = Arc::new(Mutex::new(
            OnlineReps::builder()
                .rep_weights(rep_weights.clone())
                .finish(),
        ));
        let quorum_preparer = Arc::new(VoteQuorumPreparer::new(online_reps.clone()));
        let observed_at = Timestamp::new_test_instance();
        quorum_preparer.prepare(rep.public_key(), true, observed_at);

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let network = Arc::new(RwLock::new(Network::new_test_instance()));
        let keepalive_publisher = Arc::new(KeepalivePublisher::new(
            network.clone(),
            Arc::new(PeerConnector::new_null(runtime.handle().clone())),
            MessageSender::new_null(),
            Arc::new(KeepaliveMessageFactory::new(
                network.clone(),
                Peer::new("::".to_string(), 0),
            )),
        ));
        let rep_crawler = RepCrawler::new(
            online_reps.clone(),
            quorum_preparer,
            Arc::new(Stats::default()),
            Duration::from_secs(1),
            NodeConfig::new_test_instance(),
            NetworkParams::new(rsnano_types::NetworkType::NanoDevNetwork),
            network,
            Arc::new(Ledger::new_null()),
            Arc::new(SteadyClock::new_null()),
            MessageSender::new_null(),
            keepalive_publisher,
            Arc::new(AecService::new_null()),
            runtime.handle().clone(),
        );

        let vote = ReceivedVote::new(
            Vote::new(
                &rep,
                UnixMillisTimestamp::new(123),
                0,
                vec![BlockHash::from(1)],
            )
            .into(),
            VoteSource::Live,
            Some(Arc::new(rsnano_network::Channel::new_test_instance())),
        );

        AecFactProcessor::process_vote_observation_follow_up(&rep_crawler, &vote);

        let mut online = online_reps.lock().unwrap();
        online.trim(observed_at + Duration::from_secs(60 * 10 + 1));
        assert_eq!(online.online_reps().count(), 0);
    }
}
