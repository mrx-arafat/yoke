use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use yoke_core::{config, evaluate, AgentEvent, EventKind};

/// Yoke - a harness that steers coding agents with configurable quality gates.
#[derive(Parser)]
#[command(name = "yoke", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write a starter yoke.toml in the current directory.
    Init,
    /// List the gates Yoke applies in the current directory.
    Gates,
    /// Evaluate a hook event. Reads the agent's JSON payload on stdin.
    RunHook {
        /// Event name, e.g. pre_tool_use or PreToolUse.
        event: String,
    },
}

const STARTER: &str = include_str!("../starter.toml");

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Init => init(),
        Cmd::Gates => gates(),
        Cmd::RunHook { event } => run_hook(&event),
    }
}

fn init() -> Result<()> {
    let path = Path::new("yoke.toml");
    if path.exists() {
        eprintln!("yoke.toml already exists; leaving it untouched.");
        return Ok(());
    }
    std::fs::write(path, STARTER).context("writing yoke.toml")?;
    println!("Wrote yoke.toml");
    Ok(())
}

fn gates() -> Result<()> {
    let cfg = config::load(&std::env::current_dir()?)?;
    if cfg.gates.is_empty() {
        println!("No gates configured. Run `yoke init` to start.");
        return Ok(());
    }
    let name = if cfg.meta.name.is_empty() {
        "unnamed"
    } else {
        &cfg.meta.name
    };
    println!("Gates for {name}:");
    for g in &cfg.gates {
        println!("  {:<20} on {:?}", g.name, g.on);
    }
    Ok(())
}

fn run_hook(event: &str) -> Result<()> {
    let kind = EventKind::parse(event).with_context(|| format!("unknown event '{event}'"))?;

    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .context("reading hook payload from stdin")?;
    let ev: AgentEvent = if raw.trim().is_empty() {
        AgentEvent::default()
    } else {
        serde_json::from_str(&raw).context("parsing hook payload JSON")?
    };

    let cfg = config::load(&std::env::current_dir()?)?;
    let outcome = evaluate(&cfg, kind, &ev);

    if outcome.blocked() {
        emit_block(kind, &outcome.reasons.join("; "));
    }
    // Allowed: stay silent so the agent proceeds normally.
    Ok(())
}

/// Print a veto in the agent's documented hook-output JSON shape.
fn emit_block(kind: EventKind, reason: &str) {
    let json = match kind {
        EventKind::PreToolUse => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        }),
        _ => serde_json::json!({ "decision": "block", "reason": reason }),
    };
    println!("{json}");
    eprintln!("yoke: {reason}");
}
