//! Subsystem scaffolding for upcoming `NodeServices` encapsulation.

pub mod bootstrap;
pub mod backlog;
pub mod consensus;
pub mod lifecycle;
pub mod network;
pub mod telemetry;
pub mod ticker;
pub mod wallet;

pub use backlog::BacklogSubsystem;
pub use bootstrap::{BootstrapSubsystem, BootstrapTestHandles, BootstrapWiring};
pub use consensus::ConsensusSubsystem;
pub(crate) use consensus::{ConsensusContext, ConsensusWiring};
#[cfg(any(test, feature = "test_support"))]
pub use consensus::ConsensusTestHandles;
pub use lifecycle::Lifecycle;
pub use network::NetworkSubsystem;
pub(crate) use network::NetworkWiring;
#[cfg(any(test, feature = "test_support"))]
pub use network::NetworkTestHandles;
pub use telemetry::{TelemetrySubsystem, TelemetryTestHandles, TelemetryWiring};
pub use ticker::{TickerSubsystem, TickerTestHandles};
pub use wallet::WalletSubsystem;
