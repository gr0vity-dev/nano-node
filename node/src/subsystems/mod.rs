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
pub use consensus::{ConsensusContext, ConsensusSubsystem, ConsensusTestHandles, ConsensusWiring};
pub use lifecycle::Lifecycle;
pub use network::{NetworkSubsystem, NetworkTestHandles, NetworkWiring};
pub use telemetry::{TelemetrySubsystem, TelemetryTestHandles, TelemetryWiring};
pub use ticker::{TickerSubsystem, TickerTestHandles};
pub use wallet::WalletSubsystem;
