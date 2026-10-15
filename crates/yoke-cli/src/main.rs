mod commands;
mod console;
mod diff;
mod extensions;
mod health;
mod hook;
mod install;
mod jobs;
mod memory;
mod notifications;
mod presets;
mod statusline;
mod trial;
mod update;
mod workflow;

use clap::{Parser, Subcommand, ValueEnum};
use yoke_core::event::Agent;

#[derive(Parser)]
#[command(name = "yoke", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Clone, Copy, ValueEnum)]
enum AgentArg {
    Claude,
    Codex,
}

impl From<AgentArg> for Agent {
    fn from(agent: AgentArg) -> Self {
        match agent {
            AgentArg::Claude => Self::Claude,
            AgentArg::Codex => Self::Codex,
        }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Human-only: serve the local console behind a one-time launch link; Ctrl-C stops it.
    Console {
        /// Print the one-time launch link instead of opening a browser.
        #[arg(long)]
        no_open: bool,
    },
    /// Render local status without creating or migrating state.
    Statusline {
        /// Existing statusline command to run; its output follows Yoke's line.
        #[arg(long)]
        wrap: Option<String>,
    },
    /// Create, approve and execute plans with persisted evidence.
    Plan {
        #[command(subcommand)]
        command: workflow::PlanCommand,
    },
    /// Run an independent review and store its evidence on a plan.
    Review {
        /// Plan to review; omit it to resolve the active plan from --session.
        slug: Option<String>,
        /// Recorded session id of the active plan, used when no slug is given.
        #[arg(long)]
        session: Option<String>,
        /// Agent that built the change; the other agent reviews it.
        #[arg(long, value_enum)]
        agent: AgentArg,
    },
    /// Remember a local project note after redacting recognizable secrets.
    Remember {
        /// Note to store; recognizable secrets are redacted first.
        text: String,
        /// Pin the note so session start injects it before other memories.
        #[arg(long)]
        pinned: bool,
    },
    /// Search current-project memory: best matches first, or pinned then newest without a query.
    Recall {
        /// Words to search for; any may match and the best matches come first.
        #[arg(default_value = "")]
        query: String,
        /// Most memories to list, 1 through 1000.
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=1000))]
        limit: u16,
        /// Print complete stored records as JSON.
        #[arg(long)]
        json: bool,
        /// Show what session start would inject for this query and why, within memory.inject_tokens.
        #[arg(long)]
        explain: bool,
    },
    /// Delete one locally stored memory.
    Forget {
        /// Memory id from yoke recall.
        id: i64,
    },
    /// Append a memory to project knowledge; does not trust that file.
    Share {
        /// Memory id from yoke recall.
        id: i64,
    },
    /// Pin, digest, or transfer memory as a local plaintext archive.
    Memory {
        #[command(subcommand)]
        command: memory::MemoryCommand,
    },
    /// List queued, running, failed and dead background jobs, or retry a dead one.
    Jobs {
        #[command(subcommand)]
        command: Option<jobs::JobsCommand>,
        /// Include succeeded jobs.
        #[arg(long)]
        all: bool,
        /// Print jobs as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List or append readable bundled gate configurations.
    Preset {
        #[command(subcommand)]
        command: presets::PresetCommand,
    },
    /// Write a starter yoke.toml without replacing an existing file.
    Init,
    /// List effective, trusted gates in this project.
    Gates,
    /// Read one agent hook JSON object from stdin. Allow/observe stays silent.
    RunHook {
        /// Hook event name, for example pre_tool_use or PreToolUse.
        event: String,
        /// Agent that sent the event and receives the answer.
        #[arg(long, value_enum, default_value = "claude")]
        agent: AgentArg,
    },
    /// Review and trust captured config bytes. Requires a human terminal.
    Trust {
        /// Trust the global ~/.yoke/config.toml instead of the project policy.
        #[arg(long)]
        global: bool,
    },
    /// Show effective trusted configuration, origins and snapshot freshness.
    Config {
        /// Print configuration, origins and snapshot metadata as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show newest decisions with optional exact-match filters.
    Log {
        /// Only decisions whose project root is exactly this path.
        #[arg(long)]
        project: Option<String>,
        /// Only decisions from this session id.
        #[arg(long)]
        session: Option<String>,
        /// Only decisions with this outcome, for example would_deny.
        #[arg(long)]
        outcome: Option<String>,
        /// Only decisions for this exact tool name.
        #[arg(long)]
        tool: Option<String>,
        /// Newest decisions to show, 1 through 1000.
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=1000))]
        limit: u16,
        /// Print complete stored records as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Install managed hooks. Use --dry-run to inspect all changes first.
    Install {
        /// Agents to install for, comma separated.
        #[arg(
            long,
            value_enum,
            value_delimiter = ',',
            default_value = "claude,codex"
        )]
        agent: Vec<AgentArg>,
        /// Print every planned file change without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Also install the Claude statusline, wrapping any existing command.
        #[arg(long)]
        statusline: bool,
    },
    /// Remove only hooks managed by Yoke.
    Uninstall {
        /// Agents to remove Yoke entries from, comma separated.
        #[arg(
            long,
            value_enum,
            value_delimiter = ',',
            default_value = "claude,codex"
        )]
        agent: Vec<AgentArg>,
        /// Print every planned file change without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show installation health, hook status and actionable guidance.
    Health {
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Inventory Claude Code and Codex extensions read-only; exits 1 when drift is found.
    Extensions {
        /// Print the inventory and drift as JSON, with redacted details.
        #[arg(long)]
        json: bool,
    },
    /// List actionable notifications for this project, unread first.
    Notifications {
        #[command(subcommand)]
        command: Option<notifications::NotificationsCommand>,
        /// List only unread notifications.
        #[arg(long)]
        unread: bool,
        /// Print full notification records as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Check the latest release, or verify and replace this executable.
    Update {
        /// Report the latest release version without installing anything.
        #[arg(long)]
        check: bool,
    },
    /// Report release-trial evidence from local state, read-only.
    Trial {
        #[command(subcommand)]
        command: trial::TrialCommand,
    },
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    // This escape hatch must not read stdin, discover config or touch the database.
    if args.get(1).is_some_and(|arg| arg == "run-hook")
        && std::env::var_os("YOKE_OFF").is_some_and(|value| value == "1")
    {
        return;
    }
    if args.len() == 2 && matches!(args[1].to_str(), Some("-v" | "-V" | "--version")) {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let cli = Cli::parse();
    if let Some(Cmd::RunHook { event, agent }) = &cli.cmd {
        std::process::exit(hook::run(event, (*agent).into()));
    }
    if std::env::var_os(hook::BACKGROUND).is_some() {
        std::panic::set_hook(Box::new(|info| hook::diagnostic(&format!("yoke: {info}"))));
    }
    if let Err(error) = commands::run(cli.cmd) {
        hook::diagnostic(&format!("yoke: {error:#}"));
        println!(
            "error: {}",
            commands::quote(&yoke_core::redact::redact(&format!("{error:#}")))
        );
        println!("help: \"Run yoke <command> --help for usage.\"");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn collect_missing_help(command: &clap::Command, path: &str, missing: &mut Vec<String>) {
        for arg in command.get_arguments() {
            if arg.is_hide_set() || matches!(arg.get_id().as_str(), "help" | "version") {
                continue;
            }
            if arg
                .get_help()
                .is_none_or(|help| help.to_string().trim().is_empty())
            {
                missing.push(format!("{path} {arg}"));
            }
        }
        for subcommand in command.get_subcommands() {
            if subcommand.is_hide_set() || subcommand.get_name() == "help" {
                continue;
            }
            let path = format!("{path} {}", subcommand.get_name());
            if subcommand.get_about().is_none() {
                missing.push(format!("{path} (about)"));
            }
            collect_missing_help(subcommand, &path, missing);
        }
    }

    #[test]
    fn every_visible_argument_has_help() {
        let mut command = Cli::command();
        command.build();
        let mut missing = Vec::new();
        collect_missing_help(&command, "yoke", &mut missing);
        assert!(
            missing.is_empty(),
            "help text missing for:\n{}",
            missing.join("\n")
        );
    }
}
