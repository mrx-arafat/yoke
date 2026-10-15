use anyhow::Result;
use clap::Subcommand;
use yoke_core::{config, preset};

use crate::commands;

#[derive(Subcommand)]
pub enum PresetCommand {
    /// Show bundled gate packs and plan templates.
    List,
    /// Append readable TOML; a human must trust it before any gate can run.
    Add {
        /// Preset name from yoke preset list.
        name: String,
        /// Append to the global ~/.yoke/config.toml instead of yoke.toml.
        #[arg(long)]
        global: bool,
        /// web-redirect: short instruction naming what to use for web access.
        #[arg(long)]
        replacement: Option<String>,
        /// token-saver: executable on PATH that wraps the matched commands.
        #[arg(long)]
        wrapper: Option<String>,
        /// token-saver: command words to rewrite, for example cargo test.
        #[arg(long)]
        command_prefix: Option<String>,
        /// Required for token-saver: Codex rewrites allow the changed tool call.
        #[arg(long)]
        acknowledge_rewrite_permissions: bool,
    },
}

pub fn run(command: PresetCommand) -> Result<()> {
    match command {
        PresetCommand::List => {
            let presets = preset::list();
            println!("presets[{}]{{name,description}}:", presets.len());
            for item in presets {
                println!(
                    "  {},{}",
                    commands::quote(item.name),
                    commands::quote(item.description)
                );
            }
        }
        PresetCommand::Add {
            name,
            global,
            replacement,
            wrapper,
            command_prefix,
            acknowledge_rewrite_permissions,
        } => {
            let path = if global {
                commands::home()?.join(".yoke/config.toml")
            } else {
                let cwd = std::env::current_dir()?;
                config::find_project_config(&cwd).unwrap_or_else(|| cwd.join("yoke.toml"))
            };
            let result = preset::add(
                &path,
                &name,
                &preset::PresetOptions {
                    replacement,
                    wrapper,
                    command_prefix,
                    acknowledge_rewrite_permissions,
                },
            )?;
            println!(
                "changed: {}\nfile: {}\nmessage: {}",
                result.changed,
                commands::quote(&path.display().to_string()),
                commands::quote(&result.message)
            );
        }
    }
    Ok(())
}
