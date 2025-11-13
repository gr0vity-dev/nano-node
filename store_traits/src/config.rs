#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum StoreSyncStrategy {
    /// Always flush to disk on commit. This is default.
    Always,
    /// Do not flush meta data eagerly. This may cause loss of transactions, but maintains integrity.
    NosyncSafe,
    /// Let the OS decide when to flush to disk. Guarantees depend on filesystem ordering.
    NosyncUnsafe,
    /// Use a writable memory map and let the OS flush asynchronously.
    NosyncUnsafeLargeMemory,
    /// Never sync.
    NosyncUnsafeWriteMap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerStoreConfig {
    pub sync: StoreSyncStrategy,
    pub max_databases: u32,
    pub map_size: usize,
    pub mem_init: bool,
}

impl Default for LedgerStoreConfig {
    fn default() -> Self {
        Self {
            sync: StoreSyncStrategy::Always,
            max_databases: 128,
            map_size: 256 * 1024 * 1024 * 1024,
            mem_init: false,
        }
    }
}

impl LedgerStoreConfig {
    pub fn new() -> Self {
        Self::default()
    }
}
