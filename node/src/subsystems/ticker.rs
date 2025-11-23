//! TickerSubsystem owns periodic task execution for node services. Production APIs control lifecycle; timing internals are test-only.
use std::time::Duration;

use rsnano_nullable_clock::Timestamp;
#[cfg(any(test, feature = "test_support"))]
use rsnano_utils::ticker::TickerPool;
use rsnano_utils::ticker::{Tickable, TickerScheduleData};

use crate::services::TickerServices;

use super::lifecycle::Lifecycle;

#[derive(Debug, Clone, PartialEq)]
pub struct TickerSchedule {
    pub type_name: String,
    pub interval: Duration,
    pub last_started: Option<Timestamp>,
}

impl From<TickerScheduleData> for TickerSchedule {
    fn from(data: TickerScheduleData) -> Self {
        Self {
            type_name: data.type_name,
            interval: data.interval,
            last_started: data.last_started,
        }
    }
}

pub struct TickerSubsystem {
    ticker_services: TickerServices,
}

#[cfg(any(test, feature = "test_support"))]
pub struct TickerTestHandles<'a> {
    pub ticker_pool: &'a TickerPool,
}

impl TickerSubsystem {
    pub fn new(ticker_services: TickerServices) -> Self {
        Self { ticker_services }
    }

    pub fn interval_for<T: Tickable + 'static>(&self) -> Option<Duration> {
        self.ticker_services.interval_for::<T>()
    }

    pub fn schedule_snapshot(&self) -> Vec<TickerSchedule> {
        self.ticker_services.schedule_snapshot()
    }

    #[cfg(any(test, feature = "test_support"))]
    #[doc(hidden)]
    #[deprecated(
        note = "Use ticker diagnostics via interval_for/schedule_snapshot instead of raw pool"
    )]
    pub fn ticker_pool(&self) -> &TickerPool {
        self.ticker_services.ticker_pool()
    }

    /// **Legacy test access - technical debt.**
    ///
    /// This method exposes internal subsystem components for testing.
    /// It is marked hidden and should be avoided in new tests.
    /// Phase 5 will introduce behavioral test helpers to replace this pattern.
    #[cfg(any(test, feature = "test_support"))]
    #[allow(deprecated)]
    #[doc(hidden)]
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
