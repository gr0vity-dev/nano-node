use std::{
    any::{TypeId, type_name},
    sync::{Arc, Mutex},
    time::Duration,
};

use rsnano_nullable_clock::{SteadyClock, Timestamp};

use super::Tickable;
use crate::{
    CancellationToken,
    thread_factory::{JoinHandle, ThreadFactory},
    thread_pool::ThreadPool,
};

#[derive(Debug, Clone, PartialEq)]
pub struct TickerScheduleData {
    pub type_name: String,
    pub interval: Duration,
    pub last_started: Option<Timestamp>,
}

struct TickerMetadata {
    type_id: TypeId,
    data: TickerScheduleData,
}

pub struct TickerPool {
    thread_factory: Arc<ThreadFactory>,
    tickers: Arc<Mutex<Vec<TickerState>>>,
    ticker_metadata: Arc<Mutex<Vec<TickerMetadata>>>,
    cancel_token: CancellationToken,
    main_thread: Option<JoinHandle>,
    workers: Arc<ThreadPool>,
    clock: Arc<SteadyClock>,
}

impl TickerPool {
    pub fn with_thread_pool(workers: Arc<ThreadPool>) -> Self {
        Self::new(
            workers,
            Arc::new(ThreadFactory::default()),
            CancellationToken::default(),
            Arc::new(SteadyClock::default()),
        )
    }
    pub fn new(
        workers: Arc<ThreadPool>,
        thread_factory: Arc<ThreadFactory>,
        cancel_token: CancellationToken,
        clock: Arc<SteadyClock>,
    ) -> Self {
        Self {
            thread_factory,
            tickers: Arc::new(Mutex::new(Default::default())),
            ticker_metadata: Arc::new(Mutex::new(Default::default())),
            cancel_token,
            main_thread: None,
            workers,
            clock,
        }
    }

    pub fn insert<T>(&mut self, ticker: T, interval: Duration)
    where
        T: Tickable + 'static,
    {
        self.tickers
            .lock()
            .unwrap()
            .push(TickerState::new(ticker, interval));
        self.ticker_metadata.lock().unwrap().push(TickerMetadata {
            type_id: TypeId::of::<T>(),
            data: TickerScheduleData {
                type_name: type_name::<T>().to_string(),
                interval,
                last_started: None,
            },
        });
    }

    pub fn start(&mut self) {
        let tickers = self.tickers.clone();
        let metadata = self.ticker_metadata.clone();

        let mut ticker_loop = TickerLoop::new(
            tickers,
            metadata,
            self.cancel_token.clone(),
            self.clock.clone(),
            self.workers.clone(),
        );

        self.main_thread = Some(self.thread_factory.spawn("Ticker pool", move || {
            ticker_loop.run();
        }));
    }

    pub fn stop(&mut self) {
        self.cancel_token.cancel();
        if let Some(handle) = self.main_thread.take() {
            handle.join().expect("Main ticker thread should not fail");
        }
    }

    pub fn interval_for<T: Tickable + 'static>(&self) -> Option<Duration> {
        let type_id = TypeId::of::<T>();
        self.ticker_metadata
            .lock()
            .unwrap()
            .iter()
            .find_map(|metadata| {
                if metadata.type_id == type_id {
                    Some(metadata.data.interval)
                } else {
                    None
                }
            })
    }

    pub fn schedule_snapshot(&self) -> Vec<TickerScheduleData> {
        self.ticker_metadata
            .lock()
            .unwrap()
            .iter()
            .map(|metadata| metadata.data.clone())
            .collect()
    }

    pub fn get<T: Tickable + 'static>(&self) -> Option<Duration> {
        self.tickers.lock().unwrap().iter().find_map(|s| {
            if s.ticker_type == TypeId::of::<T>() {
                Some(s.interval)
            } else {
                None
            }
        })
    }
}

struct TickerLoop {
    tickers: Arc<Mutex<Vec<TickerState>>>,
    metadata: Arc<Mutex<Vec<TickerMetadata>>>,
    cancel_token: CancellationToken,
    clock: Arc<SteadyClock>,
    workers: Arc<ThreadPool>,
}

impl TickerLoop {
    fn new(
        tickers: Arc<Mutex<Vec<TickerState>>>,
        metadata: Arc<Mutex<Vec<TickerMetadata>>>,
        cancel_token: CancellationToken,
        clock: Arc<SteadyClock>,
        workers: Arc<ThreadPool>,
    ) -> Self {
        Self {
            tickers,
            metadata,
            cancel_token,
            clock,
            workers,
        }
    }

    fn run(&mut self) {
        loop {
            let now = self.clock.now();

            self.run_one(now);

            if self
                .cancel_token
                .wait_for_cancellation(Duration::from_millis(100))
            {
                break;
            }
        }
    }

    fn run_one(&mut self, now: Timestamp) {
        let to_start = self
            .tickers
            .lock()
            .unwrap()
            .extract_if(.., |s| s.should_run(now))
            .collect::<Vec<_>>();

        for mut t in to_start {
            let last_started = Some(now);
            t.last_started = last_started;
            self.update_last_started(t.ticker_type, last_started);

            let tickers2 = self.tickers.clone();
            let cancel2 = self.cancel_token.clone();
            self.workers.execute(move || {
                t.ticker.tick(&cancel2);
                tickers2.lock().unwrap().push(t);
            });
        }
    }

    fn update_last_started(&self, type_id: TypeId, last_started: Option<Timestamp>) {
        for metadata in self
            .metadata
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|m| m.type_id == type_id)
        {
            metadata.data.last_started = last_started;
        }
    }
}

struct TickerState {
    ticker: Box<dyn Tickable>,
    ticker_type: TypeId,
    interval: Duration,
    last_started: Option<Timestamp>,
}

impl TickerState {
    fn new<T>(ticker: T, interval: Duration) -> Self
    where
        T: Tickable + 'static,
    {
        Self {
            ticker_type: TypeId::of::<T>(),
            ticker: Box::new(ticker),
            interval,
            last_started: None,
        }
    }

    fn should_run(&self, now: Timestamp) -> bool {
        match self.last_started {
            Some(t) => t.elapsed(now) >= self.interval,
            None => true,
        }
    }
}

impl Drop for TickerPool {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CancellationToken, thread_pool::ThreadPool};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn spawn_main_thread() {
        let clock = Arc::new(SteadyClock::new_null());
        let thread_factory = Arc::new(ThreadFactory::new_null());
        let spawn_tracker = thread_factory.track_spawns();
        let thread_pool = Arc::new(ThreadPool::new_null());
        let cancel_token = CancellationToken::new_null_with_uncancelled_waits(0);
        let wait_tracker = cancel_token.track_waits();
        let mut ticker_pool =
            TickerPool::new(thread_pool.clone(), thread_factory, cancel_token, clock);

        let ticker = TestTicker::new();
        let call_count = ticker.call_count.clone();
        ticker_pool.insert(ticker, Duration::from_millis(1));
        ticker_pool.start();

        let spawns = spawn_tracker.output();
        assert_eq!(spawns.len(), 1, "main thread spawn count");
        assert_eq!(spawns[0].thread_name, "Ticker pool");
        spawns[0].run();
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "call count before thread pool runs"
        );
        assert_eq!(thread_pool.queued_count(), 1, "thread pool queued");
        thread_pool.simulate();
        assert_eq!(call_count.load(Ordering::SeqCst), 1, "call count");
        let waits = wait_tracker.output();
        assert_eq!(waits.len(), 1, "waits");
        assert_eq!(waits[0], Duration::from_millis(100));
    }

    #[test]
    fn dont_call_ticker_again_if_interval_has_not_elapsed_yet() {
        let ticker = TestTicker::new();
        let call_count = ticker.call_count.clone();
        let cancel_token = CancellationToken::new_null();
        let clock = Arc::new(SteadyClock::new_null());
        let workers = Arc::new(ThreadPool::new_null());
        let tickers = Arc::new(Mutex::new(vec![TickerState::new(
            ticker,
            Duration::from_secs(60),
        )]));
        let metadata = Arc::new(Mutex::new(vec![TickerMetadata {
            type_id: TypeId::of::<TestTicker>(),
            data: TickerScheduleData {
                type_name: type_name::<TestTicker>().to_string(),
                interval: Duration::from_secs(60),
                last_started: None,
            },
        }]));
        let mut ticker_loop =
            TickerLoop::new(tickers, metadata, cancel_token, clock, workers.clone());

        let now = Timestamp::new_test_instance();
        ticker_loop.run_one(now);
        assert_eq!(workers.queued_count(), 1, "enqueued work");
        workers.simulate();
        assert_eq!(call_count.load(Ordering::SeqCst), 1, "ticker call count");

        ticker_loop.run_one(now + Duration::from_secs(1));
        assert_eq!(workers.queued_count(), 0, "second enqueued work");

        ticker_loop.run_one(now + Duration::from_secs(60));
        assert_eq!(workers.queued_count(), 1, "third enqueued work");
    }

    struct TestTicker {
        call_count: Arc<AtomicUsize>,
    }

    impl TestTicker {
        fn new() -> Self {
            Self {
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl Tickable for TestTicker {
        fn tick(&mut self, cancel_token: &CancellationToken) {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            cancel_token.cancel();
        }
    }
}
