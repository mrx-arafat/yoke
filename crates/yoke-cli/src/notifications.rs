use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::Serialize;
use yoke_core::config::{self, LoadedConfig};
use yoke_core::notifications::{self, Notification, Target};
use yoke_core::store::Store;

use crate::commands::{self, quote};

#[derive(Subcommand)]
pub enum NotificationsCommand {
    /// Human-only: acknowledge one notification by id, or every unread one with `all`.
    Ack {
        /// Notification id, or all for every unread one in scope.
        target: String,
    },
}

const PENDING: &str = "Older history is still being read in short steps so hooks are not held up; run yoke notifications again to see all of it.";

#[derive(Serialize)]
struct Item<'a> {
    #[serde(flatten)]
    notification: &'a Notification,
    next: String,
    doc: &'static str,
}

/// The CLI step that inspects a notification's subject.
pub(crate) fn next_command(notification: &Notification) -> String {
    match notification.kind.as_str() {
        "hook_error" => "yoke log --outcome error --limit 10".into(),
        "observe_would_deny" => "yoke log --outcome would_deny --limit 20".into(),
        "plan_waiting" | "plan_stalled" | "check_failed" => {
            format!("yoke plan status {}", notification.subject)
        }
        _ => "yoke config".into(),
    }
}

/// Existing state and trusted config, without creating or migrating anything.
fn open_current() -> Result<Option<(Store, LoadedConfig)>> {
    let home = commands::home()?;
    let Some(store) = commands::open_current(&home)? else {
        return Ok(None);
    };
    let loaded = config::load_trusted(&store, &std::env::current_dir()?, &home)?;
    Ok(Some((store, loaded)))
}

pub fn run(command: Option<NotificationsCommand>, unread: bool, json: bool) -> Result<()> {
    let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
    if let Some(NotificationsCommand::Ack { target }) = command {
        // Acknowledgement hides an alert from the human, so an agent must not do it.
        commands::require_human("notifications ack")?;
        let target = match target.as_str() {
            "all" => Target::All,
            id => Target::Ids(vec![id
                .parse()
                .context("ack target must be a notification id or all")?]),
        };
        let (store, loaded) =
            open_current()?.context("no notifications: Yoke state does not exist yet")?;
        let changed = notifications::acknowledge(&store, &loaded.project_root, &target, now)?;
        println!("acknowledged: {changed}");
        return Ok(());
    }
    let Some((store, loaded)) = open_current()? else {
        if json {
            println!("[]");
        } else {
            println!("notifications[0]{{id,state,kind,count,last_seen,title,next,doc}}:\nunread: 0\nhelp: \"No Yoke state exists yet; the first hook event creates it.\"");
        }
        return Ok(());
    };
    // One bounded call per run: looping would start each next write transaction at once and
    // starve hooks that are waiting for the lock.
    let pending = notifications::sync(&store, &loaded, now)?;
    let rows = notifications::list(&store, &loaded.project_root, unread, 200)?;
    if json {
        let items: Vec<_> = rows
            .iter()
            .map(|notification| Item {
                notification,
                next: next_command(notification),
                doc: notifications::help_for(&notification.kind).doc,
            })
            .collect();
        println!("{}", serde_json::to_string(&items)?);
        if pending {
            eprintln!("yoke: {PENDING}");
        }
        return Ok(());
    }
    println!(
        "notifications[{}]{{id,state,kind,count,last_seen,title,next,doc}}:",
        rows.len()
    );
    for row in &rows {
        println!(
            "  {},{},{},{},{},{},{},{}",
            row.id,
            quote(&row.state),
            quote(&row.kind),
            row.count,
            row.last_seen,
            quote(&row.title),
            quote(&next_command(row)),
            quote(notifications::help_for(&row.kind).doc)
        );
    }
    println!(
        "unread: {}",
        notifications::unread_count(&store, &loaded.project_root)?
    );
    if pending {
        println!("pending: true\nhelp: {}", quote(PENDING));
    } else {
        println!("help: \"Acknowledge from your own terminal with yoke notifications ack <id|all>; add --json for full records.\"");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    #[test]
    fn every_help_and_next_command_parses_with_the_real_cli() {
        let mut commands: Vec<String> = yoke_core::notifications::HELP
            .iter()
            .flat_map(|topic| topic.commands.iter().map(|command| command.to_string()))
            .collect();
        for kind in [
            "hook_error",
            "observe_would_deny",
            "plan_stalled",
            "config_stale",
        ] {
            commands.push(super::next_command(&sample(kind)));
        }
        for command in commands {
            let args = command.split_whitespace().map(|word| {
                match word.strip_prefix('<').and_then(|w| w.strip_suffix('>')) {
                    Some(placeholder) => placeholder.split('|').next().unwrap().to_owned(),
                    None => word.to_owned(),
                }
            });
            assert!(
                crate::Cli::try_parse_from(args).is_ok(),
                "help command does not parse: {command}"
            );
        }
    }

    fn sample(kind: &str) -> yoke_core::notifications::Notification {
        yoke_core::notifications::Notification {
            id: 1,
            kind: kind.into(),
            project: String::new(),
            subject: "demo".into(),
            plan_id: None,
            title: String::new(),
            detail: String::new(),
            count: 1,
            first_seen: 0,
            last_seen: 0,
            state: "unread".into(),
            acknowledged_at: None,
            actor: None,
            resolved_at: None,
            help: "",
        }
    }
}
