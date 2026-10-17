mod bypass;
mod commands;
mod console;
mod diff;
mod extensions;
mod health;
mod hook;
mod install;
mod jobs;
mod log_cmd;
mod memory;
mod notifications;
mod presets;
mod preview;
mod retention;
mod starter;
mod statusline;
mod trial;
mod update;
mod usage_cmd;
mod watch;
mod workflow;

use std::io::IsTerminal;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
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
        /// Print the one-time launch link instead of opening a browser; use it on a server.
        #[arg(long)]
        no_open: bool,
        /// Listen on this 127.0.0.1 port instead of a random free one, so an SSH tunnel can be set up first.
        #[arg(long, default_value_t = 0)]
        port: u16,
    },
    /// Render local status without creating or migrating the state database.
    Statusline {
        /// Existing statusline command to run; its output follows Yoke's line.
        #[arg(long)]
        wrap: Option<String>,
        /// How much to show: one line, or a second line with the model and last event.
        #[arg(long, value_enum, default_value_t)]
        style: statusline::Style,
        /// Internal: refresh the cached git count for this directory, then exit.
        #[arg(
            long,
            hide = true,
            value_name = "DIR",
            conflicts_with = "refresh_health"
        )]
        refresh_git: Option<std::path::PathBuf>,
        /// Internal: refresh the cached health problem for this directory, then exit.
        #[arg(long, hide = true, value_name = "DIR")]
        refresh_health: Option<std::path::PathBuf>,
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
        /// Print the full report as JSON, including the reviewer's raw output.
        #[arg(long)]
        json: bool,
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
        /// Show what session start injects in enforce mode for this query and why, within memory.inject_tokens; says when the current mode injects nothing.
        #[arg(long)]
        explain: bool,
    },
    /// Delete one memory of this project, and its entry in project knowledge if it was shared.
    Forget {
        /// Memory id from yoke recall in this project.
        id: i64,
    },
    /// Append a memory to project knowledge; does not trust that file.
    Share {
        /// Memory id from yoke recall in this project.
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
    /// Write a starter yoke.toml at the project root without replacing an existing file.
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
    /// Show the newest decisions of this project, or of every project with --all; optional exact-match filters.
    Log {
        /// Only decisions for the project that contains this path; a path outside any project matches as written.
        #[arg(long)]
        project: Option<String>,
        /// Show decisions from every project instead of the current one.
        #[arg(long, conflicts_with = "project")]
        all: bool,
        /// Only decisions from this session id; searched in every project unless --project is given.
        #[arg(long)]
        session: Option<String>,
        /// Only decisions with this outcome: allow, deny, would_deny, rewrite or error.
        #[arg(long)]
        outcome: Option<String>,
        /// Only decisions for this tool name; the match is exact and case-sensitive.
        #[arg(long)]
        tool: Option<String>,
        /// Newest decisions to show, 1 through 1000.
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=1000))]
        limit: u16,
        /// Print stored records as JSON; payloads are cut at log.payload_cap and flagged by payload_truncated.
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
        /// Install the Claude statusline only when none is set; keep any other command.
        #[arg(long, conflicts_with = "statusline")]
        statusline_if_absent: bool,
        /// With --statusline or --statusline-if-absent: compact is one line, detailed adds a second.
        #[arg(long, value_enum)]
        style: Option<statusline::Style>,
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
    /// Inventory Claude Code and Codex extensions read-only; exits 1 when a stale or dangling definition is found.
    Extensions {
        /// Print the inventory, drift and every note as JSON, with redacted details.
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
    /// Show agent quota, recent token usage and cost estimates, and the newest sessions; read-only.
    Usage {
        /// Most sessions to list, 1 through 100.
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Live read-only view of this project's decisions, quota, plans and health; q quits. Without a terminal it prints one snapshot.
    Watch,
    /// Show the token that lets you start an agent with Yoke's hooks skipped, or pause every hook for a limited time. Human-only.
    Pause {
        /// Skip every hook for this long, for example 30m or 2h (at most 24h).
        #[arg(long = "for", value_name = "DURATION", conflicts_with = "clear")]
        duration: Option<String>,
        /// End a pause early.
        #[arg(long)]
        clear: bool,
    },
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    // The bypass must not read stdin, discover config or touch the database. Only the per-install
    // token or a pause set by a human skips the hook; YOKE_OFF=1 and similar values are refused.
    if args.get(1).is_some_and(|arg| arg == "run-hook") && bypass::skips_hook() {
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
    // Rust ignores SIGPIPE, so `yoke log | head` would panic on the closed pipe. Restore the
    // default so output ends quietly like any Unix tool; the console serves sockets, not a pipe.
    #[cfg(unix)]
    if !matches!(cli.cmd, Some(Cmd::Console { .. })) {
        // SAFETY: resetting a signal disposition before any thread is started.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
    }
    let result = match cli.cmd {
        // Reads recorded state only: no trusted project configuration is needed or loaded.
        Some(Cmd::Usage { limit, json }) => usage_cmd::run(json, limit),
        // Opens existing state read-only and keeps running without it, like `yoke usage`.
        Some(Cmd::Watch) => watch::run(),
        command => commands::run(command),
    };
    if let Err(error) = result {
        // A person at a terminal reads stdout, so the stderr copy is only for logs and pipes.
        if !std::io::stdout().is_terminal() {
            hook::diagnostic(&format!("yoke: {error:#}"));
        }
        println!(
            "error: {}",
            commands::quote(&yoke_core::redact::redact(&format!("{error:#}")))
        );
        println!(
            "help: {}",
            commands::quote(&format!("Run {} --help for usage.", command_path(&args)))
        );
        std::process::exit(1);
    }
}

/// `yoke plan new` for `yoke plan new login --title x`: the deepest subcommand the arguments name.
fn command_path(args: &[std::ffi::OsString]) -> String {
    let mut command = Cli::command();
    let mut path = vec!["yoke".to_owned()];
    for arg in args.iter().skip(1) {
        let Some(name) = arg.to_str() else { break };
        if name.starts_with('-') {
            continue;
        }
        let Some(sub) = command.find_subcommand(name).cloned() else {
            break;
        };
        path.push(name.to_owned());
        command = sub;
    }
    path.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_error_help_line_names_the_subcommand_that_failed() {
        let path = |args: &[&str]| {
            let args: Vec<_> = args.iter().map(std::ffi::OsString::from).collect();
            command_path(&args)
        };
        let new_plan = ["yoke", "plan", "new", "login", "--title", "x"];
        assert_eq!(path(&new_plan), "yoke plan new");
        assert_eq!(path(&["yoke", "log", "--outcome", "plan"]), "yoke log");
        assert_eq!(path(&["yoke"]), "yoke");
    }

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
