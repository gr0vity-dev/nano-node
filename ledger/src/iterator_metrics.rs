use std::sync::Arc;

use rsnano_utils::stats::{DetailType, Direction, StatType, Stats};

use crate::StoreIterator;

#[derive(Clone, Debug)]
pub struct IteratorMetricsConfig {
    pub enabled: bool,
}

impl Default for IteratorMetricsConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
}

#[derive(Clone)]
pub(crate) struct LedgerIteratorMetrics {
    stats: Arc<Stats>,
    config: IteratorMetricsConfig,
}

impl LedgerIteratorMetrics {
    pub fn new(stats: Arc<Stats>, config: IteratorMetricsConfig) -> Self {
        Self { stats, config }
    }

    pub fn instrument<'a, T>(
        &self,
        iter: StoreIterator<'a, T>,
        category: IteratorMetricKind,
    ) -> StoreIterator<'a, T>
    where
        T: 'a,
    {
        if !self.config.enabled {
            return iter;
        }

        Box::new(InstrumentedIterator::new(
            iter,
            Arc::clone(&self.stats),
            category.detail(),
        ))
    }
}

#[derive(Clone, Copy)]
pub enum IteratorMetricKind {
    AccountRange,
    AccountFullScan,
    PendingRange,
    BlockRange,
    ReceivableRange,
}

impl IteratorMetricKind {
    fn detail(self) -> DetailType {
        match self {
            IteratorMetricKind::AccountRange => DetailType::LedgerIteratorAccountRange,
            IteratorMetricKind::AccountFullScan => DetailType::LedgerIteratorAccountFull,
            IteratorMetricKind::PendingRange => DetailType::LedgerIteratorPendingRange,
            IteratorMetricKind::BlockRange => DetailType::LedgerIteratorBlockRange,
            IteratorMetricKind::ReceivableRange => DetailType::LedgerIteratorReceivable,
        }
    }
}

struct InstrumentedIterator<'a, T> {
    inner: StoreIterator<'a, T>,
    stats: Arc<Stats>,
    detail: DetailType,
    produced: u64,
}

impl<'a, T> InstrumentedIterator<'a, T> {
    fn new(inner: StoreIterator<'a, T>, stats: Arc<Stats>, detail: DetailType) -> Self {
        stats.inc_dir(StatType::LedgerIterator, detail, Direction::In);
        Self {
            inner,
            stats,
            detail,
            produced: 0,
        }
    }
}

impl<'a, T> Iterator for InstrumentedIterator<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.inner.next();
        if item.is_some() {
            self.produced += 1;
        }
        item
    }
}

impl<'a, T> Drop for InstrumentedIterator<'a, T> {
    fn drop(&mut self) {
        if self.produced == 0 {
            return;
        }

        self.stats.add_dir(
            StatType::LedgerIterator,
            self.detail,
            Direction::Out,
            self.produced,
        );
    }
}
