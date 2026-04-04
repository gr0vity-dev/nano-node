use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use rsnano_utils::stats::{StatsCollection, StatsSource};

#[derive(Default)]
pub(crate) struct OptimisticSchedulerStats {
    pub loop_count: AtomicU64,
    pub activated_count: AtomicU64,
    pub insert_count: AtomicU64,
    pub insert_failed_count: AtomicU64,
}

impl StatsSource for OptimisticSchedulerStats {
    fn collect_stats(&self, result: &mut StatsCollection) {
        result.insert(
            "optimistic_scheduler",
            "loop",
            self.loop_count.load(Relaxed),
        );
        result.insert(
            "optimistic_scheduler",
            "activated",
            self.activated_count.load(Relaxed),
        );
        result.insert(
            "optimistic_scheduler",
            "insert",
            self.insert_count.load(Relaxed),
        );
        result.insert(
            "optimistic_scheduler",
            "insert_failed",
            self.insert_failed_count.load(Relaxed),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_stats() {
        let stats = OptimisticSchedulerStats {
            loop_count: AtomicU64::new(3),
            activated_count: AtomicU64::new(5),
            insert_count: AtomicU64::new(7),
            insert_failed_count: AtomicU64::new(9),
        };
        let mut collection = StatsCollection::default();
        stats.collect_stats(&mut collection);
        assert_eq!(collection.get("optimistic_scheduler", "loop"), 3);
        assert_eq!(collection.get("optimistic_scheduler", "activated"), 5);
        assert_eq!(collection.get("optimistic_scheduler", "insert"), 7);
        assert_eq!(collection.get("optimistic_scheduler", "insert_failed"), 9);
    }
}
