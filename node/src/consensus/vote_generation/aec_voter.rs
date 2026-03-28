use std::{sync::Arc, time::Duration};

use rsnano_nullable_clock::SteadyClock;
use rsnano_types::{BlockHash, NetworkType, Root};
use rsnano_utils::{CancellationToken, ticker::Tickable};

use super::{CpsLimiter, VoteGenerators};
use crate::consensus::{
    AecService, election::VoteType, election_schedulers::priority::bucket_count,
};

/// Creates votes for blocks within the AEC
pub(crate) struct AecVoter {
    aec: Arc<AecService>,
    vote_generators: Arc<VoteGenerators>,
    clock: Arc<SteadyClock>,
    cps_limiter: CpsLimiter,
    current_bucket: usize,
    vote_broadcast_interval: Duration,
}

impl AecVoter {
    pub(crate) fn new(
        aec: Arc<AecService>,
        vote_generators: Arc<VoteGenerators>,
        clock: Arc<SteadyClock>,
        network: NetworkType,
        cps_limiter: CpsLimiter,
    ) -> Self {
        Self {
            aec,
            vote_generators,
            clock,
            cps_limiter,
            current_bucket: bucket_count() - 1,
            vote_broadcast_interval: match network {
                NetworkType::NanoDevNetwork => Duration::from_millis(500),
                _ => Duration::from_secs(15),
            },
        }
    }

    fn flush(&self, queue: &mut Vec<(Root, BlockHash, VoteType)>) {
        // TODO: enqueue with one call
        for (root, hash, vote_type) in queue.drain(..) {
            self.vote_generators.generate_vote(&root, &hash, vote_type);
        }
    }
}

impl Tickable for AecVoter {
    fn tick(&mut self, cancel_token: &CancellationToken) {
        let now = self.clock.now();
        let mut voted = true;
        let mut vote_queue = Vec::new();
        while voted {
            voted = false;
            loop {
                let vote_target = self.aec.next_vote_in_bucket(
                    self.current_bucket,
                    self.vote_broadcast_interval,
                    now,
                );

                if let Some((root, vote_type, winner_hash)) = vote_target {
                    if vote_type == VoteType::NonFinal && !self.cps_limiter.try_vote(now) {
                        self.flush(&mut vote_queue);
                        return;
                    }

                    vote_queue.push((root.root, winner_hash, vote_type));

                    self.aec.set_last_voted(&root, vote_type, now);
                    voted = true;
                }

                if cancel_token.is_cancelled() {
                    return;
                }

                if self.current_bucket == 0 {
                    self.current_bucket = bucket_count() - 1;
                    break;
                } else {
                    self.current_bucket -= 1;
                }
            }
        }
        self.flush(&mut vote_queue);
    }
}
