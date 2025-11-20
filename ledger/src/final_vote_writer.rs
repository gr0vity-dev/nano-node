use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

#[derive(Default)]
pub struct FinalVoteWriterStats {
    optimistic_successes: AtomicU64,
    optimistic_conflicts: AtomicU64,
    pessimistic_fallbacks: AtomicU64,
    optimistic_active: AtomicU64,
    max_optimistic_concurrency: AtomicU64,
}

impl FinalVoteWriterStats {
    pub fn optimistic_successes(&self) -> u64 {
        self.optimistic_successes.load(Relaxed)
    }

    pub fn optimistic_conflicts(&self) -> u64 {
        self.optimistic_conflicts.load(Relaxed)
    }

    pub fn pessimistic_fallbacks(&self) -> u64 {
        self.pessimistic_fallbacks.load(Relaxed)
    }

    pub fn max_optimistic_concurrency(&self) -> u64 {
        self.max_optimistic_concurrency.load(Relaxed)
    }

    pub(crate) fn start_optimistic_writer(&self) -> OptimisticWriterGuard<'_> {
        let active = self.optimistic_active.fetch_add(1, Relaxed) + 1;
        let mut observed = self.max_optimistic_concurrency.load(Relaxed);
        while active > observed {
            match self
                .max_optimistic_concurrency
                .compare_exchange(observed, active, Relaxed, Relaxed)
            {
                Ok(_) => break,
                Err(actual) => observed = actual,
            }
        }
        OptimisticWriterGuard { stats: self }
    }

    pub fn add_deltas(&self, successes: u64, conflicts: u64, fallbacks: u64) {
        self.optimistic_successes.fetch_add(successes, Relaxed);
        self.optimistic_conflicts.fetch_add(conflicts, Relaxed);
        self.pessimistic_fallbacks.fetch_add(fallbacks, Relaxed);
    }

    fn end_optimistic_writer(&self) {
        self.optimistic_active.fetch_sub(1, Relaxed);
    }
}

pub(crate) struct OptimisticWriterGuard<'a> {
    stats: &'a FinalVoteWriterStats,
}

impl Drop for OptimisticWriterGuard<'_> {
    fn drop(&mut self) {
        self.stats.end_optimistic_writer();
    }
}
