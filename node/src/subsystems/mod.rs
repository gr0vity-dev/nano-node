//! Subsystem scaffolding for upcoming `NodeServices` encapsulation.

pub mod lifecycle;
pub mod network;
pub mod consensus;
pub mod bootstrap;
pub mod telemetry;
pub mod ticker;

pub use lifecycle::Lifecycle;
