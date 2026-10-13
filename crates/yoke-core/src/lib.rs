//! Core engine for Yoke: config loading, agent events, and gate evaluation.

pub mod config;
pub mod engine;
pub mod event;

pub use config::{Config, Gate};
pub use engine::{evaluate, Outcome};
pub use event::{AgentEvent, EventKind};
