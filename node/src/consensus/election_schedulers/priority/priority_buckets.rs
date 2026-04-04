use std::{
    ops::{Deref, DerefMut},
    sync::LazyLock,
};

use crate::consensus::{
    BucketInfo, ElectionCandidate, ElectionCandidateSource,
    election_schedulers::priority::{
        Bucket, BucketInsertError, Bucketing, Eviction, PriorityBucketConfig,
        bucket_stats::BucketStats, prio_bucket_index,
    },
};
use rsnano_types::{BlockHash, BlockPriority, SavedBlock};
use rsnano_utils::stats::{StatsCollection, StatsSource};

pub(crate) struct PriorityBuckets {
    buckets: Vec<Bucket>,
    activations_per_bucket: Vec<u64>,
    // TODO remove this:
    pub bucket_stats: BucketStats,
}

impl PriorityBuckets {
    pub fn new(bucket_count: usize, config: PriorityBucketConfig) -> Self {
        let mut buckets = Vec::with_capacity(bucket_count);
        for bucket_id in 0..bucket_count {
            buckets.push(Bucket::new(config.clone(), bucket_id));
        }

        Self {
            buckets,
            activations_per_bucket: vec![0; bucket_count],
            bucket_stats: BucketStats::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.buckets.iter().map(|b| b.len()).sum()
    }

    pub fn contains(&self, hash: &BlockHash) -> bool {
        self.buckets.iter().any(|b| b.contains(hash))
    }

    pub fn insert(
        &mut self,
        priority: BlockPriority,
        block: SavedBlock,
    ) -> Result<Eviction, BucketInsertError> {
        let index = prio_bucket_index(priority.balance);
        self.activations_per_bucket[index] += 1;
        self.buckets[index].insert(priority, block)
    }
}

impl ElectionCandidateSource for PriorityBuckets {
    fn should_schedule(&self, buckets: &[BucketInfo]) -> bool {
        self.buckets.iter().any(|b| b.available(buckets))
    }

    fn gather_candidates(&mut self, buckets: &[BucketInfo], result: &mut Vec<ElectionCandidate>) {
        for bucket in self.buckets.iter_mut().rev() {
            bucket.activate(buckets, result);
        }
    }
}

impl Deref for PriorityBuckets {
    type Target = Vec<Bucket>;

    fn deref(&self) -> &Self::Target {
        &self.buckets
    }
}

impl DerefMut for PriorityBuckets {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.buckets
    }
}

impl StatsSource for PriorityBuckets {
    fn collect_stats(&self, result: &mut StatsCollection) {
        for (i, activations) in self.activations_per_bucket.iter().enumerate() {
            result.insert("election_bucket_activation", &BUCKET_NAMES[i], *activations);
        }
    }
}

static BUCKET_NAMES: LazyLock<Vec<String>> = LazyLock::new(|| {
    let bucket_count = Bucketing::new().bucket_count();
    let mut names = Vec::with_capacity(bucket_count);
    for i in 0..bucket_count {
        names.push(i.to_string())
    }
    names
});
