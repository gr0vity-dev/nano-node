//! TickerSubsystem owns periodic task execution for node services. Surface is lifecycle plus diagnostics (`interval_for`/`schedule_snapshot`); no raw TickerPool exposure in prod or tests.
use std::time::Duration;

use rsnano_nullable_clock::Timestamp;
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
}

impl Lifecycle for TickerSubsystem {
    fn start(&mut self) {
        self.ticker_services.start();
    }

    fn stop(&mut self) {
        self.ticker_services.stop();
    }
}
