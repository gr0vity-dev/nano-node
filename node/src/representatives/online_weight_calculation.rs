use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tracing::info;

use rsnano_nullable_clock::SteadyClock;
use rsnano_utils::{CancellationToken, ticker::Tickable};

use super::{OnlineReps, OnlineWeightSampler, VoteQuorumPreparer};

pub struct OnlineWeightCalculation {
    sampler: OnlineWeightSampler,
    online_reps: Arc<Mutex<OnlineReps>>,
    quorum_preparer: Arc<VoteQuorumPreparer>,
    clock: Arc<SteadyClock>,
    first_run: bool,
    last_sample: Instant,
}

impl OnlineWeightCalculation {
    pub fn new(
        sampler: OnlineWeightSampler,
        online_reps: Arc<Mutex<OnlineReps>>,
        quorum_preparer: Arc<VoteQuorumPreparer>,
        clock: Arc<SteadyClock>,
    ) -> Self {
        Self {
            sampler,
            online_reps,
            quorum_preparer,
            clock,
            first_run: true,
            last_sample: Instant::now(),
        }
    }

    fn calculate_trended_weight(&mut self) {
        let result = self.sampler.calculate_trend();
        info!(
            "Trended weight updated: {}, samples: {}",
            result.trended.format_balance(0),
            result.sample_count
        );
        self.online_reps.lock().unwrap().set_trended(result.trended);
        self.quorum_preparer.set_trended(result.trended);
    }
}

impl Tickable for OnlineWeightCalculation {
    fn tick(&mut self, _: &CancellationToken) {
        if self.first_run {
            // Don't sample online weight on first run, because it is always 0
            self.sampler.sanitize();
            self.last_sample = Instant::now();
            self.calculate_trended_weight();
            self.first_run = false;
        } else {
            {
                let mut online = self.online_reps.lock().unwrap();
                online.trim(self.clock.now());
                online.calculate_online_weight();
            }
            self.quorum_preparer.trim(self.clock.now());
            if self.last_sample.elapsed() > Duration::from_secs(60) {
                let online_weight = self.quorum_preparer.online_weight();
                self.sampler.add_sample(online_weight);
                self.calculate_trended_weight();
                self.last_sample = Instant::now();
            }
        }
    }
}
