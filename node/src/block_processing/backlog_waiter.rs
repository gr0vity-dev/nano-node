use std::{
    cmp::min,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering::Relaxed},
    },
    time::Duration,
};

use tracing::warn;

use rsnano_ledger::Ledger;
use rsnano_nullable_clock::{SteadyClock, Timestamp};

use super::BlockProcessorQueue;
use rsnano_utils::stats::{StatsCollection, StatsSource};

/// Waits for the backlog to fall below the backlog limit
pub(crate) struct BacklogWaiter {
    queue: Arc<BlockProcessorQueue>,
    ledger: Arc<Ledger>,
    max_backlog: u64,
    call_count: AtomicUsize,
    cooldown_count: AtomicUsize,
    last_log: Mutex<Option<Timestamp>>,
    clock: Arc<SteadyClock>,
}

impl BacklogWaiter {
    pub fn new(
        queue: Arc<BlockProcessorQueue>,
        ledger: Arc<Ledger>,
        clock: Arc<SteadyClock>,
        max_backlog: u64,
    ) -> Self {
        Self {
            queue,
            ledger,
            max_backlog,
            call_count: AtomicUsize::new(0),
            cooldown_count: AtomicUsize::new(0),
            last_log: Mutex::new(None),
            clock,
        }
    }

    #[allow(dead_code)]
    pub fn new_null() -> Self {
        let queue = Arc::new(BlockProcessorQueue::new_null());
        let ledger = Arc::new(Ledger::new_null(
            rsnano_store_lmdb::null_ledger_store_factory(),
        ));
        let clock = Arc::new(SteadyClock::new_null());
        Self::new(queue, ledger, clock, 1000)
    }

    pub fn wait_for_backlog(&self) {
        self.call_count.fetch_add(1, Relaxed);
        let backlog_count = self.ledger.backlog_count();
        let throttle_wait = throttle_wait(backlog_count, self.max_backlog);
        if throttle_wait.is_zero() {
            return;
        }

        let now = self.clock.now();
        if self.should_log(now) {
            warn!(
                throttle_ms = throttle_wait.as_millis(),
                backlog_size = backlog_count,
                "Backlog exceeded. Throttling!"
            );
        }

        self.cooldown_count.fetch_add(1, Relaxed);
        self.queue.wait(throttle_wait);
    }

    fn should_log(&self, now: Timestamp) -> bool {
        let mut last_log = self.last_log.lock().unwrap();
        let should_log = match *last_log {
            Some(i) => i.elapsed(now) >= Duration::from_secs(15),
            None => true,
        };

        if should_log {
            *last_log = Some(now);
        }

        should_log
    }

    #[allow(dead_code)]
    pub fn call_count(&self) -> usize {
        self.call_count.load(Relaxed)
    }
}

impl StatsSource for BacklogWaiter {
    fn collect_stats(&self, result: &mut StatsCollection) {
        result.insert(
            "block_processor",
            "cooldown_backlog",
            self.cooldown_count.load(Relaxed),
        );
    }
}

fn throttle_wait(backlog_count: u64, max_backlog: u64) -> Duration {
    const BACKLOG_THROTTLE_MS: u64 = 100;
    const BACKLOG_THROTTLE_MAX_MS: u64 = 1000;

    let backlog_factor = backlog_factor(backlog_count, max_backlog);

    if backlog_factor < 1.0 {
        return Duration::ZERO;
    }

    // This uses a power of approximately 3.32, which gives ~1x at 1.0 and ~10x at 2.0
    let scaling = backlog_factor.powf(3.32);
    let throttle_wait_ms = min(
        (BACKLOG_THROTTLE_MS as f64 * scaling) as u64,
        BACKLOG_THROTTLE_MAX_MS,
    );

    Duration::from_millis(throttle_wait_ms)
}

fn backlog_factor(backlog_count: u64, max_backlog: u64) -> f64 {
    const BACKLOG_THRESHOLD: f64 = 1.5;

    if max_backlog == 0 || backlog_count <= max_backlog {
        return 0.0;
    }

    let max_with_threshold = max_backlog as f64 * BACKLOG_THRESHOLD;
    let factor = backlog_count as f64 / max_with_threshold;
    factor
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_output_tracker::OutputTrackerMt;
    use tracing_test::traced_test;

    #[test]
    fn test_backlog_factor() {
        fn assert_factor(backlog_count: u64, max_backlog: u64, expected: f64) {
            let factor = backlog_factor(backlog_count, max_backlog);
            let delta = (expected - factor).abs();
            assert!(delta < 0.01, "expected: {}, actual: {}", expected, factor);
        }

        assert_factor(0, 0, 0.0);
        assert_factor(1000, 0, 0.0);
        assert_factor(0, 1000, 0.0);
        assert_factor(999, 1000, 0.0);
        assert_factor(1000, 1000, 0.0);
        assert_factor(1001, 1000, 0.667);
        assert_factor(1250, 1000, 0.833);
        assert_factor(1500, 1000, 1.0);
        assert_factor(2000, 1000, 1.333);
        assert_factor(3000, 1000, 2.0);
    }

    #[test]
    fn test_throttle_wait() {
        fn assert_throttle(backlog_count: u64, max_backlog: u64, expected_ms: u64) {
            let throttle = throttle_wait(backlog_count, max_backlog);
            assert_eq!(throttle, Duration::from_millis(expected_ms));
        }

        assert_throttle(0, 0, 0);
        assert_throttle(1000, 1000, 0);
        assert_throttle(1499, 1000, 0);
        assert_throttle(1500, 1000, 100);
        assert_throttle(2000, 1000, 259);
        assert_throttle(2500, 1000, 545);
        assert_throttle(3000, 1000, 998);
        assert_throttle(10000, 1000, 1000);
    }

    #[test]
    fn dont_wait_when_no_backlog() {
        let TestFixture {
            waiter,
            wait_tracker,
        } = create_fixture(FixtureArgs {
            confirmed: 5000,
            unconfirmed: 0,
            ..Default::default()
        });

        waiter.wait_for_backlog();

        assert_eq!(wait_tracker.output(), vec![]);
    }

    #[test]
    fn wait_when_backlog_over_limit() {
        let TestFixture {
            waiter,
            wait_tracker,
        } = create_fixture(FixtureArgs {
            confirmed: 5000,
            unconfirmed: 3000,
            ..Default::default()
        });

        waiter.wait_for_backlog();

        assert_eq!(
            wait_tracker.output(),
            vec![throttle_wait(3000, MAX_BACKLOG)]
        );
    }

    #[test]
    fn stats_source() {
        let TestFixture { waiter, .. } = create_fixture(Default::default());

        waiter.wait_for_backlog();

        let mut stats = StatsCollection::new();
        waiter.collect_stats(&mut stats);

        assert_eq!(stats.get("block_processor", "cooldown_backlog"), 1);
    }

    #[test]
    #[traced_test]
    fn log_initial() {
        let TestFixture { waiter, .. } = create_fixture(Default::default());

        waiter.wait_for_backlog();

        logs_assert(|logs| {
            if logs.len() != 1 {
                return Err(format!("len was {}, expected 1", logs.len()));
            }
            if !logs[0].contains("Backlog exceeded. Throttling! throttle_ms=998 backlog_size=3000")
            {
                return Err(logs[0].to_owned());
            }
            Ok(())
        });
    }

    #[test]
    #[traced_test]
    fn suppress_logs_for_15_secs() {
        let clock =
            SteadyClock::new_null_with_offsets([Duration::from_secs(14), Duration::from_secs(1)]);

        let TestFixture { waiter, .. } = create_fixture(FixtureArgs {
            clock,
            ..Default::default()
        });

        waiter.wait_for_backlog();
        waiter.wait_for_backlog();
        waiter.wait_for_backlog();

        logs_assert(|logs| {
            if logs.len() != 2 {
                Err(format!("Expected 2 log entries, but found: {}", logs.len()))
            } else {
                Ok(())
            }
        });
    }

    #[test]
    fn can_track_waits() {
        let TestFixture { waiter, .. } = create_fixture(Default::default());

        waiter.wait_for_backlog();
        assert_eq!(waiter.call_count(), 1);

        waiter.wait_for_backlog();
        assert_eq!(waiter.call_count(), 2);
    }

    fn create_fixture(args: FixtureArgs) -> TestFixture {
        let queue = Arc::new(BlockProcessorQueue::new_null());
        let ledger = Arc::new(Ledger::new_null(
            rsnano_store_lmdb::null_ledger_store_factory(),
        ));

        ledger
            .store
            .cache()
            .confirmed_count
            .store(args.confirmed, Relaxed);

        ledger
            .store
            .cache()
            .block_count
            .store(args.confirmed + args.unconfirmed, Relaxed);

        let wait_tracker = queue.track_waits();
        let waiter = BacklogWaiter::new(queue, ledger, args.clock.into(), MAX_BACKLOG);

        TestFixture {
            waiter,
            wait_tracker,
        }
    }

    struct FixtureArgs {
        confirmed: u64,
        unconfirmed: u64,
        clock: SteadyClock,
    }

    impl Default for FixtureArgs {
        fn default() -> Self {
            Self {
                confirmed: 5000,
                unconfirmed: 3000,
                clock: SteadyClock::new_null(),
            }
        }
    }

    struct TestFixture {
        waiter: BacklogWaiter,
        wait_tracker: Arc<OutputTrackerMt<Duration>>,
    }

    const MAX_BACKLOG: u64 = 1000;
}
