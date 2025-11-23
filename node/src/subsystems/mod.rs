//! Subsystem scaffolding for upcoming `NodeServices` encapsulation.

pub mod backlog;
pub mod bootstrap;
pub mod consensus;
pub mod lifecycle;
pub mod network;
pub mod telemetry;
pub mod ticker;
pub mod wallet;

pub use backlog::BacklogSubsystem;
#[cfg(any(test, feature = "test_support"))]
pub use bootstrap::BootstrapTestHandles;
pub use bootstrap::{BootstrapSubsystem, BootstrapWiring};
pub use consensus::ConsensusSubsystem;
#[cfg(any(test, feature = "test_support"))]
pub use consensus::ConsensusTestHandles;
pub(crate) use consensus::{ConsensusContext, ConsensusWiring};
pub use lifecycle::Lifecycle;
pub use network::NetworkSubsystem;
#[cfg(any(test, feature = "test_support"))]
pub use network::NetworkTestHandles;
pub(crate) use network::NetworkWiring;
#[cfg(any(test, feature = "test_support"))]
pub use telemetry::TelemetryTestHandles;
pub use telemetry::{TelemetrySubsystem, TelemetryWiring};
pub use ticker::TickerSubsystem;
#[cfg(any(test, feature = "test_support"))]
pub use ticker::TickerTestHandles;
pub use wallet::WalletSubsystem;
