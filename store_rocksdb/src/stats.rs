use parking_lot::RwLock;
use rsnano_utils::stats::Stats;
use std::sync::{Arc, Weak};

static GLOBAL_STATS: RwLock<Option<Weak<Stats>>> = RwLock::new(None);

pub fn register_rocksdb_stats(stats: Arc<Stats>) {
    *GLOBAL_STATS.write() = Some(Arc::downgrade(&stats));
}

pub fn get_stats_handle() -> Option<Arc<Stats>> {
    GLOBAL_STATS.read().as_ref().and_then(|weak| weak.upgrade())
}
