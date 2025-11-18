use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[cfg(test)]
use crate::ledger_factory::default_ledger_store_factory;
use rsnano_ledger::Ledger;
use rsnano_network::{Channel, ChannelId};
use rsnano_nullable_clock::SteadyClock;
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use rsnano_types::{BlockHash, Networks, Root, SavedBlock};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    stats::{DetailType, StatType, Stats},
};

use super::{LocalVoteHistory, vote_generator::VoteGenerator};
use crate::{
    config::{NetworkParams, NodeConfig},
    consensus::{VoteBroadcaster, election::VoteType},
    transport::MessageSender,
    wallets::WalletRepresentatives,
};

#[derive(Clone)]
pub struct VoteGenerationEvent {
    pub channel_id: ChannelId,
    pub blocks: Vec<SavedBlock>,
    pub final_vote: bool,
}

pub struct VoteGenerators {
    non_final_vote_generator: VoteGenerator,
    final_vote_generator: VoteGenerator,
    vote_listener: OutputListenerMt<VoteGenerationEvent>,
    voting_delay: Duration,
    wallet_reps: Arc<Mutex<WalletRepresentatives>>,
    stats: Arc<Stats>,
}

impl VoteGenerators {
    fn voting_delay_for(network: Networks) -> Duration {
        match network {
            Networks::NanoDevNetwork => Duration::from_secs(1),
            _ => Duration::from_secs(15),
        }
    }

    pub(crate) fn new(
        ledger: Arc<Ledger>,
        wallet_reps: Arc<Mutex<WalletRepresentatives>>,
        history: Arc<LocalVoteHistory>,
        stats: Arc<Stats>,
        config: &NodeConfig,
        network_params: &NetworkParams,
        vote_broadcaster: Arc<VoteBroadcaster>,
        message_sender: MessageSender,
        clock: Arc<SteadyClock>,
    ) -> Self {
        let voting_delay = Self::voting_delay_for(network_params.network.current_network);

        let non_final_vote_generator = VoteGenerator::new(
            ledger.clone(),
            wallet_reps.clone(),
            history.clone(),
            false, //none-final
            stats.clone(),
            message_sender.clone(),
            voting_delay,
            config.vote_generator_delay,
            vote_broadcaster.clone(),
            clock.clone(),
        );

        let final_vote_generator = VoteGenerator::new(
            ledger,
            wallet_reps.clone(),
            history,
            true, //final
            stats.clone(),
            message_sender.clone(),
            voting_delay,
            config.vote_generator_delay,
            vote_broadcaster,
            clock,
        );

        Self {
            non_final_vote_generator,
            final_vote_generator,
            vote_listener: OutputListenerMt::new(),
            voting_delay,
            wallet_reps,
            stats,
        }
    }

    #[cfg(test)]
    pub fn new_null() -> Self {
        let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));
        let wallet_reps = Arc::new(Mutex::new(WalletRepresentatives::new_null()));
        let history = Arc::new(LocalVoteHistory::new(Networks::NanoLiveNetwork));
        let stats = Arc::new(Stats::default());
        let config = NodeConfig::new_test_instance();
        let network_params = NetworkParams::new(Networks::NanoLiveNetwork);
        let vote_broadcaster = Arc::new(VoteBroadcaster::new_null());
        let message_sender = MessageSender::new_null();
        let clock = Arc::new(SteadyClock::new_null());
        Self::new(
            ledger,
            wallet_reps,
            history,
            stats,
            &config,
            &network_params,
            vote_broadcaster,
            message_sender,
            clock,
        )
    }

    pub fn voting_delay(&self) -> Duration {
        self.voting_delay
    }

    pub fn start(&self) {
        self.non_final_vote_generator.start();
        self.final_vote_generator.start();
    }

    pub fn stop(&self) {
        self.non_final_vote_generator.stop();
        self.final_vote_generator.stop();
    }

    pub fn track(&self) -> Arc<OutputTrackerMt<VoteGenerationEvent>> {
        self.vote_listener.track()
    }

    pub fn generate_vote(&self, root: &Root, hash: &BlockHash, vote_type: VoteType) {
        match vote_type {
            VoteType::NonFinal => {
                self.stats
                    .inc(StatType::Election, DetailType::GenerateVoteNormal);
                self.non_final_vote_generator.add(root, hash);
            }
            VoteType::Final => {
                self.stats
                    .inc(StatType::Election, DetailType::GenerateVoteFinal);
                self.final_vote_generator.add(root, hash);
            }
        }
    }

    pub(crate) fn generate_votes(
        &self,
        blocks: &[SavedBlock],
        channel: &Arc<Channel>,
        vote_type: VoteType,
    ) -> usize {
        if self.vote_listener.is_tracked() {
            self.vote_listener.emit(VoteGenerationEvent {
                channel_id: channel.channel_id(),
                blocks: blocks.to_vec(),
                final_vote: vote_type == VoteType::Final,
            });
        }

        match vote_type {
            VoteType::NonFinal => self.non_final_vote_generator.generate(blocks, channel),
            VoteType::Final => self.final_vote_generator.generate(blocks, channel),
        }
    }

    pub fn voting_enabled(&self) -> bool {
        self.wallet_reps.lock().unwrap().voting_enabled()
    }
}

impl ContainerInfoProvider for VoteGenerators {
    fn container_info(&self) -> ContainerInfo {
        ContainerInfo::builder()
            .node("non_final", self.non_final_vote_generator.container_info())
            .node("final", self.final_vote_generator.container_info())
            .finish()
    }
}
