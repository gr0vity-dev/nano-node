use rsnano_utils::ticker::TickerPool;

use crate::services::TickerServices;

use super::lifecycle::Lifecycle;

pub struct TickerSubsystem {
    ticker_services: TickerServices,
}

pub struct TickerTestHandles<'a> {
    pub ticker_pool: &'a TickerPool,
}

impl TickerSubsystem {
    pub fn new(ticker_services: TickerServices) -> Self {
        Self { ticker_services }
    }

    pub fn ticker_pool(&self) -> &TickerPool {
        self.ticker_services.ticker_pool()
    }

    pub fn test_handles(&self) -> TickerTestHandles<'_> {
        TickerTestHandles {
            ticker_pool: self.ticker_services.ticker_pool(),
        }
    }
}

impl Lifecycle for TickerSubsystem {
    fn start(&mut self) {
        self.ticker_services.start();
    }

    fn stop(&mut self) {
        self.ticker_services.stop();
    }
}
