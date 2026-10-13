//! Core engine for Yoke: config loading, agent events, and gate evaluation.

pub mod config;
pub mod engine;
pub mod event;
pub mod guard;
pub mod memory;
pub mod plan;
pub mod preset;
pub mod redact;
pub mod review;
pub mod stop_guard;
pub mod store;
pub mod trust;
pub mod usage;
pub mod wire;

pub use config::{Config, Gate};
pub use engine::{evaluate, Outcome};
pub use event::{AgentEvent, EventKind};
