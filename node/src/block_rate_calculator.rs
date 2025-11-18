use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use rsnano_ledger::Ledger;
use rsnano_nullable_clock::SteadyClock;
use rsnano_utils::{CancellationToken, ticker::Tickable};
#[cfg(test)]
use store_rocksdb::default_ledger_store_factory;

use crate::utils::RateCalculator;

#[derive(Default)]
pub struct CurrentBlockRates {
    bps: AtomicI64,
    cps: AtomicI64,
}

impl CurrentBlockRates {
    /// Checked blocks per second. Can be negative due to bounded backlog rollbacks
    pub fn bps(&self) -> i64 {
        self.bps.load(Ordering::Relaxed)
    }

    /// Confirmed blocks per second
    pub fn cps(&self) -> i64 {
        self.cps.load(Ordering::Relaxed)
    }

    pub fn set_bps(&self, value: i64) {
        self.bps.store(value, Ordering::Relaxed);
    }

    pub fn set_cps(&self, value: i64) {
        self.cps.store(value, Ordering::Relaxed);
    }
}

pub(crate) struct BlockRateCalculator {
    clock: Arc<SteadyClock>,
    ledger: Arc<Ledger>,
    current_rates: Arc<CurrentBlockRates>,
    bps_calculator: RateCalculator,
    cps_calculator: RateCalculator,
}

impl BlockRateCalculator {
    pub fn new(clock: Arc<SteadyClock>, ledger: Arc<Ledger>) -> Self {
        Self {
            clock,
            ledger,
            current_rates: Arc::new(CurrentBlockRates::default()),
            bps_calculator: RateCalculator::new(),
            cps_calculator: RateCalculator::new(),
        }
    }

    pub fn rates(&self) -> &Arc<CurrentBlockRates> {
        &self.current_rates
    }
}

impl Tickable for BlockRateCalculator {
    fn tick(&mut self, _cancel_token: &CancellationToken) {
        let now = self.clock.now();
        self.bps_calculator.sample(self.ledger.block_count(), now);
        self.cps_calculator
            .sample(self.ledger.confirmed_count(), now);
        self.current_rates.set_bps(self.bps_calculator.rate());
        self.current_rates.set_cps(self.cps_calculator.rate());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn initial_state() {
        let clock = Arc::new(SteadyClock::new_null());
        let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));

        let calculator = BlockRateCalculator::new(clock, ledger);

        assert_rates(calculator, 0, 0);
    }

    #[test]
    fn run_with_no_change() {
        let clock = Arc::new(SteadyClock::new_null());
        let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));
        let mut calculator = BlockRateCalculator::new(clock, ledger);

        calculator.tick(&CancellationToken::new_null());

        assert_rates(calculator, 0, 0);
    }

    #[test]
    fn two_samples() {
        let clock = Arc::new(SteadyClock::new_null_with_offsets([Duration::from_millis(
            500,
        )]));
        let ledger = Arc::new(Ledger::new_null(default_ledger_store_factory()));
        let mut calculator = BlockRateCalculator::new(clock, ledger.clone());

        calculator.tick(&CancellationToken::new_null());

        ledger.simulate_block_count(126);
        ledger.simulate_confirmed_count(101);
        calculator.tick(&CancellationToken::new_null());

        assert_rates(calculator, 250, 200);
    }

    fn assert_rates(calculator: BlockRateCalculator, expected_bps: i64, expected_cps: i64) {
        let rates = calculator.rates();
        assert_eq!(rates.bps(), expected_bps, "BPS");
        assert_eq!(rates.cps(), expected_cps, "CPS");
    }
}
