//! Core engine for Yoke: config loading, agent events, and gate evaluation.

pub mod config;
pub mod engine;
pub mod event;
pub mod extensions;
pub mod guard;
pub mod jobs;
pub mod memory;
pub mod notifications;
pub mod plan;
pub mod preset;
pub mod private;
pub mod process;
pub mod project;
pub mod quota;
pub mod redact;
pub mod review;
pub mod stop_guard;
pub mod store;
pub mod timefmt;
pub mod trial;
pub mod trust;
pub mod usage;
pub mod wire;

pub use config::{Config, Gate};
pub use engine::{evaluate, Outcome};
pub use event::{AgentEvent, EventKind};
