mod backlog_index;
mod backlog_scan;
mod backlog_waiter;
mod block_batch_processor;
mod block_context;
mod block_processor;
mod block_processor_queue;
mod bounded_backlog;
mod bounded_backlog_plugin;
mod local_block_broadcaster;
mod process_queue;
mod unchecked_map;

use rsnano_ledger::{BlockError, RollbackResults};
use rsnano_types::{Block, BlockHash, SavedBlock};

pub use backlog_scan::{BacklogScan, BacklogScanConfig};
pub(crate) use backlog_waiter::BacklogWaiter;
pub use block_context::*;
pub use block_processor::*;
pub(crate) use block_processor_queue::*;
pub use bounded_backlog::*;
pub(crate) use bounded_backlog_plugin::*;
pub(crate) use local_block_broadcaster::*;
pub use process_queue::ProcessQueueConfig;
use rsnano_utils::stats::DetailType;
use strum_macros::{EnumCount, EnumIter, IntoStaticStr};
pub use unchecked_map::*;

pub enum LedgerEvent {
    /// The confirmed block + it's confirmation root
    BlocksProcessed(Vec<ProcessedResult>),
    BlocksConfirmed(Vec<(SavedBlock, BlockHash)>),
    BlocksRolledBack(RollbackResults),
    ConfirmationFailed(BlockHash),
    ConfirmingSetNearFull,
    ConfirmingSetRecovered,
}

#[derive(Clone, Debug)]
pub struct ProcessedResult {
    pub block: Block,
    pub source: BlockSource,
    pub status: Result<(), BlockError>,
    pub saved_block: Option<SavedBlock>,
}

#[derive(
    Copy, Clone, PartialEq, Eq, Debug, PartialOrd, Ord, EnumIter, EnumCount, Hash, IntoStaticStr,
)]
#[repr(u8)]
#[strum(serialize_all = "snake_case")]
pub enum BlockSource {
    Unknown = 0,
    Live,
    LiveOriginator,
    Bootstrap,
    BootstrapLegacy,
    Unchecked,
    Local,
    Forced,
    Election,
}

impl From<BlockSource> for DetailType {
    fn from(value: BlockSource) -> Self {
        match value {
            BlockSource::Unknown => DetailType::Unknown,
            BlockSource::Live => DetailType::Live,
            BlockSource::LiveOriginator => DetailType::LiveOriginator,
            BlockSource::Bootstrap => DetailType::Bootstrap,
            BlockSource::BootstrapLegacy => DetailType::BootstrapLegacy,
            BlockSource::Unchecked => DetailType::Unchecked,
            BlockSource::Local => DetailType::Local,
            BlockSource::Forced => DetailType::Forced,
            BlockSource::Election => DetailType::Election,
        }
    }
}

impl BlockSource {
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn as_str(&self) -> &'static str {
        self.into()
    }
}
