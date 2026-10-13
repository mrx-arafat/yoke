mod commands;
mod console;
mod hook;
mod install;
mod memory;
mod presets;
mod statusline;
mod update;
mod workflow;

use clap::{Parser, Subcommand, ValueEnum};
use yoke_core::event::Agent;

/// Local, trusted quality gates for coding agents.
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
    /// Serve the local console; Ctrl-C stops the server.
    Console {
        #[arg(long)]
        no_open: bool,
    },
    /// Render local status without creating or migrating state.
    Statusline {
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
        slug: Option<String>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long, value_enum)]
        agent: AgentArg,
    },
    /// Remember a local project note after redacting recognizable secrets.
    Remember {
        text: String,
        #[arg(long)]
        pinned: bool,
    },
    /// Search current-project memory, with pinned results first.
    Recall {
        #[arg(default_value = "")]
        query: String,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=1000))]
        limit: u16,
        #[arg(long)]
        json: bool,
    },
    /// Delete one locally stored memory.
    Forget { id: i64 },
    /// Append a memory to project knowledge; does not trust that file.
    Share { id: i64 },
    /// Pin, digest, or transfer memory as a local plaintext archive.
    Memory {
        #[command(subcommand)]
        command: memory::MemoryCommand,
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
        event: String,
        #[arg(long, value_enum, default_value = "claude")]
        agent: AgentArg,
    },
    /// Review and trust captured config bytes. Requires a human terminal.
    Trust {
        #[arg(long)]
        global: bool,
    },
    /// Show effective trusted configuration, origins and snapshot freshness.
    Config {
        #[arg(long)]
        json: bool,
    },
    /// Show newest decisions with optional exact-match filters.
    Log {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        outcome: Option<String>,
        #[arg(long)]
        tool: Option<String>,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=1000))]
        limit: u16,
        #[arg(long)]
        json: bool,
    },
    /// Install managed hooks. Use --dry-run to inspect all changes first.
    Install {
        #[arg(
            long,
            value_enum,
            value_delimiter = ',',
            default_value = "claude,codex"
        )]
        agent: Vec<AgentArg>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        statusline: bool,
    },
    /// Remove only hooks managed by Yoke.
    Uninstall {
        #[arg(
            long,
            value_enum,
            value_delimiter = ',',
            default_value = "claude,codex"
        )]
        agent: Vec<AgentArg>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Check the latest release, or verify and replace this executable.
    Update {
        #[arg(long)]
        check: bool,
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
    if let Err(error) = commands::run(cli.cmd) {
        eprintln!("yoke: {}", yoke_core::redact::redact(&format!("{error:#}")));
        println!(
            "error: {}",
            commands::quote(&yoke_core::redact::redact(&format!("{error:#}")))
        );
        println!("help: \"Run yoke <command> --help for usage.\"");
        std::process::exit(1);
    }
}
