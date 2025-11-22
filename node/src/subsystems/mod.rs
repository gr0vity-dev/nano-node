//! Subsystem scaffolding for upcoming `NodeServices` encapsulation.

pub mod bootstrap;
pub mod consensus;
pub mod lifecycle;
pub mod network;
pub mod telemetry;
pub mod ticker;

pub use bootstrap::{BootstrapSubsystem, BootstrapTestHandles};
pub use consensus::{ConsensusSubsystem, ConsensusTestHandles, ConsensusWiring};
pub use lifecycle::Lifecycle;
pub use network::{NetworkSubsystem, NetworkTestHandles, NetworkWiring};
pub use telemetry::{TelemetrySubsystem, TelemetryTestHandles};
pub use ticker::{TickerSubsystem, TickerTestHandles};
