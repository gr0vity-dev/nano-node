use std::sync::atomic::{AtomicU64, Ordering};

use rsnano_utils::stats::{StatsCollection, StatsSource};
use strum::{EnumCount, IntoEnumIterator};

use rsnano_types::VoteSource;

pub(crate) struct VoteCounter {
    votes: AtomicU64,
    by_source: [AtomicU64; VoteSource::COUNT],
}

impl Default for VoteCounter {
    fn default() -> Self {
        Self {
            votes: AtomicU64::new(0),
            by_source: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl VoteCounter {
    #[allow(dead_code)]
    pub fn votes(&self) -> u64 {
        self.votes.load(Ordering::Relaxed)
    }

    #[allow(dead_code)]
    pub fn votes_by(&self, source: VoteSource) -> u64 {
        self.by_source[source as usize].load(Ordering::Relaxed)
    }

    pub fn count(&self, source: VoteSource) {
        self.votes.fetch_add(1, Ordering::Relaxed);
        self.by_source[source as usize].fetch_add(1, Ordering::Relaxed);
    }
}

impl StatsSource for VoteCounter {
    fn collect_stats(&self, result: &mut StatsCollection) {
        result.insert("election", "vote", self.votes());
        for source in VoteSource::iter() {
            result.insert("election_vote", source.as_str(), self.votes_by(source));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_counted() {
        let counter = VoteCounter::default();
        assert_eq!(counter.votes(), 0);
        assert_eq!(counter.votes_by(VoteSource::Live), 0);
        assert_eq!(counter.votes_by(VoteSource::Cache), 0);
        assert_eq!(counter.votes_by(VoteSource::Rebroadcast), 0);
    }

    #[test]
    fn count_one_vote() {
        let counter = VoteCounter::default();

        counter.count(VoteSource::Live);

        assert_eq!(counter.votes(), 1);
        assert_eq!(counter.votes_by(VoteSource::Live), 1);
        assert_eq!(counter.votes_by(VoteSource::Cache), 0);
        assert_eq!(counter.votes_by(VoteSource::Rebroadcast), 0);
    }

    #[test]
    fn count_multiple_votes() {
        let counter = VoteCounter::default();

        counter.count(VoteSource::Live);
        counter.count(VoteSource::Live);
        counter.count(VoteSource::Rebroadcast);

        assert_eq!(counter.votes(), 3);
        assert_eq!(counter.votes_by(VoteSource::Live), 2);
        assert_eq!(counter.votes_by(VoteSource::Cache), 0);
        assert_eq!(counter.votes_by(VoteSource::Rebroadcast), 1);
    }

    #[test]
    fn collect_stats() {
        let mut stats = StatsCollection::new();
        let counter = VoteCounter::default();
        counter.count(VoteSource::Live);
        counter.count(VoteSource::Live);
        counter.count(VoteSource::Rebroadcast);

        counter.collect_stats(&mut stats);

        assert_eq!(stats.get("election", "vote"), 3);
        assert_eq!(stats.get("election_vote", "live"), 2);
        assert_eq!(stats.get("election_vote", "rebroadcast"), 1);
        assert_eq!(stats.get("election_vote", "cache"), 0);
    }
}
