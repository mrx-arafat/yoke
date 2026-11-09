//! Editing workflow settings from the console.
//! A save edits `yoke.toml` or `~/.yoke/config.toml` with `toml_edit`, so comments and layout
//! stay, and only the keys whose value differs from the effective one. The human first sees
//! the exact diff; the file is written only if it still has the bytes this page was rendered
//! from and the diff is the one that was reviewed. Nothing is ever trusted here: the edit
//! applies after the human runs `yoke trust` (`--global`) in a terminal. Gates stay file-only.
use super::super::{actions::Outcome, Console};
use anyhow::{anyhow, bail, ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};
use toml_edit::{DocumentMut, Item, Table, TableLike};
use yoke_core::{
    config::{self, Config, LoadedConfig},
    store::Store,
    team_remote::{side_by_side, RowKind},
};

/// Largest config file Yoke trusts (`trust.rs` refuses bigger ones).
const LIMIT: u64 = 1024 * 1024;

pub(super) enum Kind {
    Bool,
    /// A whole number of 0 or more; `Config::validate` checks the upper ranges.
    Count,
    Text,
    Choice(&'static [&'static str]),
    /// A model name for an agent CLI Yoke launches; empty means unset (the CLI default).
    Model,
}

pub(super) struct Field {
    pub(super) key: &'static str,
    pub(super) kind: Kind,
    pub(super) help: &'static str,
}

const AGENTS: &[&str] = &["claude", "codex"];

/// Every non-gate setting in `yoke_core::config::Config`.
pub(super) const FIELDS: [Field; 16] = [
    Field { key: "mode", kind: Kind::Choice(&["observe", "enforce"]), help: "observe records would-deny decisions; enforce lets gates deny, rewrite and block Stop." },
    Field { key: "review.reviewer", kind: Kind::Choice(&["auto", "claude", "codex"]), help: "Agent CLI Yoke launches for independent review; auto picks the agent that is not building." },
    Field { key: "review.models.claude", kind: Kind::Model, help: "Model Yoke asks for when Claude Code reviews (--model). Empty means the CLI default." },
    Field { key: "review.models.codex", kind: Kind::Model, help: "Model Yoke asks for when Codex reviews (-m). Empty means the CLI default." },
    Field { key: "memory.digest", kind: Kind::Bool, help: "Whether Yoke launches an agent to digest session facts into memories." },
    Field { key: "memory.digest_agent", kind: Kind::Choice(AGENTS), help: "Agent CLI Yoke launches for the memory digest." },
    Field { key: "memory.digest_models.claude", kind: Kind::Model, help: "Model Yoke asks for when Claude Code digests memory. Empty means the CLI default." },
    Field { key: "memory.digest_models.codex", kind: Kind::Model, help: "Model Yoke asks for when Codex digests memory. Empty means the CLI default." },
    Field { key: "memory.inject_tokens", kind: Kind::Count, help: "Token budget for memory context injected at session start in enforce mode." },
    Field { key: "stop.max_blocks", kind: Kind::Count, help: "Stop attempts without progress before an active plan stalls." },
    Field { key: "plans.dir", kind: Kind::Text, help: "Where new plan files are created, relative to the project." },
    Field { key: "log.retention_days", kind: Kind::Count, help: "Days of decisions, facts, finished check runs and notifications that are kept." },
    Field { key: "log.payload_cap", kind: Kind::Count, help: "Bytes kept from each redacted hook payload." },
    Field { key: "update.check", kind: Kind::Bool, help: "Check GitHub for a stable release at most once a day at session start; yoke update --check still works when this is off." },
    Field { key: "usage.warn_percent", kind: Kind::Count, help: "Warn when an account quota window reaches this percent used (1 to 100). Warnings are advisory." },
    Field { key: "meta.name", kind: Kind::Text, help: "Project label." },
];

/// The value `config` gives `key`, as the form shows it; `None` for an unset model.
pub(super) fn effective(config: &Config, key: &str) -> Option<String> {
    Some(match key {
        "mode" => config.mode.as_str().into(),
        "review.reviewer" => config.review.reviewer.clone(),
        "review.models.claude" => return config.review.models.claude.clone(),
        "review.models.codex" => return config.review.models.codex.clone(),
        "memory.digest" => config.memory.digest.to_string(),
        "memory.digest_agent" => config.memory.digest_agent.clone(),
        "memory.digest_models.claude" => return config.memory.digest_models.claude.clone(),
        "memory.digest_models.codex" => return config.memory.digest_models.codex.clone(),
        "memory.inject_tokens" => config.memory.inject_tokens.to_string(),
        "stop.max_blocks" => config.stop.max_blocks.to_string(),
        "plans.dir" => config.plans.dir.clone(),
        "log.retention_days" => config.log.retention_days.to_string(),
        "log.payload_cap" => config.log.payload_cap.to_string(),
        "update.check" => config.update.check.to_string(),
        "usage.warn_percent" => config.usage.warn_percent.to_string(),
        "meta.name" => config.meta.name.clone(),
        _ => return None,
    })
}

/// The project and global config files this console's policy is read from.
pub(super) fn files(loaded: &LoadedConfig) -> Result<(PathBuf, PathBuf)> {
    let find = |scope: &str| {
        loaded
            .snapshots
            .iter()
            .find(|s| s.scope == scope && s.file.file_name().is_some_and(|n| n != "knowledge.md"))
            .map(|s| s.file.clone())
            .with_context(|| format!("Yoke found no {scope} config file location"))
    };
    Ok((find("project")?, find("global")?))
}

/// What the settings become once the files on disk are trusted. A file that is missing,
/// unreadable or invalid cannot be trusted, so its trusted snapshot stays in effect.
pub(super) fn after_trust(loaded: &LoadedConfig, project: &Path, global: &Path) -> Config {
    let mut config = Config::default();
    for file in [global, project] {
        let trusted = loaded
            .policy_sources
            .iter()
            .find(|row| Path::new(&row.file) == file)
            .and_then(|row| Config::parse(&row.content).ok());
        let disk = read_target(file)
            .ok()
            .flatten()
            .and_then(|text| Config::parse(&text).ok())
            .filter(|config| config.check_hook_budgets().is_ok());
        if let Some(layer) = disk.or(trusted) {
            config.merge(layer);
        }
    }
    config
}

/// Read a config file the console may edit: `None` when absent; refuses links, non-regular
/// files, files over 1 MiB and non-UTF-8 text.
fn read_target(file: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(file) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(anyhow!("Cannot inspect {}: {error}", file.display())),
        Ok(meta) => {
            ensure!(!meta.file_type().is_symlink(), "{} is a symbolic link; the console edits only regular files, so change it in your own editor", file.display());
            ensure!(
                meta.is_file() && meta.len() <= LIMIT,
                "{} must be a regular file of at most 1 MiB to edit it here",
                file.display()
            );
        }
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut text = String::new();
    options
        .open(file)
        .and_then(|f| f.take(LIMIT + 1).read_to_string(&mut text))
        .map_err(|error| anyhow!("Cannot read {} as UTF-8 text: {error}", file.display()))?;
    ensure!(
        text.len() as u64 <= LIMIT,
        "{} grew beyond 1 MiB",
        file.display()
    );
    Ok(Some(text))
}

fn fingerprint(text: Option<&str>) -> String {
    text.map_or_else(
        || "absent".into(),
        |text| format!("{:x}", Sha256::digest(text.as_bytes())),
    )
}

/// The file version a page shows, sent back on save so an edit made since is never lost.
pub(super) fn page_fingerprint(file: &Path) -> String {
    read_target(file).map_or_else(|_| "unreadable".into(), |text| fingerprint(text.as_deref()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Project,
    Global,
}

struct Proposal {
    scope: Scope,
    file: PathBuf,
    base: String,
    before: Option<String>,
    after: String,
    keys: Vec<String>,
}

/// The form value as text (for comparing with `effective`) and as a TOML value; `None` unsets.
fn desired(field: &Field, raw: &str) -> Result<(Option<String>, Option<toml_edit::Value>)> {
    let (key, raw) = (field.key, raw.trim());
    Ok(match field.kind {
        Kind::Bool => match raw {
            "true" => (Some(raw.into()), Some(true.into())),
            "false" => (Some(raw.into()), Some(false.into())),
            _ => bail!("{key} must be true or false"),
        },
        Kind::Count => {
            let number = raw
                .parse::<i64>()
                .ok()
                .filter(|n| *n >= 0)
                .with_context(|| format!("{key} must be a whole number of 0 or more"))?;
            (Some(number.to_string()), Some(number.into()))
        }
        Kind::Choice(options) => {
            ensure!(
                options.contains(&raw),
                "{key} must be one of {}",
                options.join(", ")
            );
            (Some(raw.into()), Some(raw.into()))
        }
        Kind::Text => (Some(raw.into()), Some(raw.into())),
        Kind::Model if raw.is_empty() => (None, None),
        Kind::Model => (Some(raw.into()), Some(raw.into())),
    })
}

/// Set or remove `key` (dotted) in `doc`, keeping the decor of a replaced value (its inline
/// comment). Returns whether the key was there before.
fn set(
    doc: &mut DocumentMut,
    key: &str,
    value: Option<toml_edit::Value>,
    file: &Path,
) -> Result<bool> {
    let parts: Vec<&str> = key.split('.').collect();
    let (leaf, parents) = parts.split_last().context("empty setting name")?;
    let not_table = |part: &str| {
        anyhow!(
            "{part} in {} is not a table; change {key} in your own editor",
            file.display()
        )
    };
    let mut table: &mut dyn TableLike = doc.as_table_mut();
    for part in parents {
        if !table.contains_key(part) {
            if value.is_none() {
                return Ok(false);
            }
            let mut new = Table::new();
            new.set_implicit(true);
            table.insert(part, Item::Table(new));
        }
        table = table
            .get_mut(part)
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| not_table(part))?;
    }
    let present = table.contains_key(leaf);
    match (value, table.get_mut(leaf)) {
        (None, _) => {
            table.remove(leaf);
        }
        (Some(value), Some(item)) => {
            let existing = item.as_value_mut().with_context(|| {
                format!(
                    "{key} in {} is a table, not a value; change it in your own editor",
                    file.display()
                )
            })?;
            let decor = existing.decor().clone();
            *existing = value;
            *existing.decor_mut() = decor;
        }
        (Some(value), None) => {
            table.insert(leaf, Item::Value(value));
        }
    }
    Ok(present)
}

fn propose(loaded: &LoadedConfig, params: &Value) -> Result<Proposal> {
    let scope = match params["target"].as_str() {
        Some("project") => Scope::Project,
        Some("global") => Scope::Global,
        _ => bail!("Choose the project file or the global file to save to"),
    };
    let (project, global) = files(loaded)?;
    let file = if scope == Scope::Project {
        project.clone()
    } else {
        global
    };
    let base = params["base"]
        .as_str()
        .context("Missing the file version this page showed; reload the page")?
        .to_owned();
    let values = params["values"]
        .as_object()
        .context("Missing setting values")?;
    let before = read_target(&file)?;
    ensure!(
        fingerprint(before.as_deref()) == base,
        "{} changed since this page was loaded; reload the page and make your edit again",
        file.display()
    );
    let mut doc: DocumentMut = before.as_deref().unwrap_or("").parse().map_err(|error| {
        anyhow!(
            "{} is not valid TOML ({error}); fix it in your own editor, then reload this page",
            file.display()
        )
    })?;
    let project_source = project.display().to_string();
    let mut keys = Vec::new();
    for (key, raw) in values {
        let field = FIELDS.iter().find(|f| f.key == key).with_context(|| {
            format!(
                "{key} cannot be edited here; gates and environment allowances stay in the file"
            )
        })?;
        let raw = raw
            .as_str()
            .with_context(|| format!("{key} must be sent as text"))?;
        let (wanted, value) = desired(field, raw)?;
        if wanted == effective(&loaded.config, key) {
            continue;
        }
        let origin = loaded
            .origins
            .get(key.as_str())
            .map_or("default", String::as_str);
        ensure!(
            scope == Scope::Project || origin != project_source,
            "{key} is set in {project_source}, which overrides the global file; save it to the project file instead"
        );
        let unset = value.is_none();
        let present = set(&mut doc, key, value, &file)?;
        ensure!(
            present || !unset,
            "{key} is not set in {}, so there is nothing to unset there; its value comes from {origin}",
            file.display()
        );
        keys.push(key.clone());
    }
    let after = doc.to_string();
    if !keys.is_empty() {
        Config::parse(&after)
            .and_then(|config| config.check_hook_budgets())
            .map_err(|error| {
                let error = format!("{error:#}");
                let reason = error
                    .split("; edit the policy file")
                    .next()
                    .unwrap_or(&error);
                anyhow!(
                    "yoke trust would refuse {} with this change, so nothing was written: {reason}",
                    file.display()
                )
            })?;
    }
    Ok(Proposal {
        scope,
        file,
        base,
        before,
        after,
        keys,
    })
}

/// Unified diff lines with two lines of context; `...` marks skipped unchanged lines.
fn diff(before: &str, after: &str) -> String {
    let Some(rows) = side_by_side(before, after) else {
        return "The file is longer than 2000 lines, so it is not compared line by line.\n".into();
    };
    let changed: Vec<bool> = rows.iter().map(|row| row.kind != RowKind::Same).collect();
    let near = |i: usize| (i.saturating_sub(2)..(i + 3).min(rows.len())).any(|j| changed[j]);
    let mut out = String::new();
    let mut skipped = false;
    for (i, row) in rows.iter().enumerate() {
        if !near(i) {
            skipped = true;
            continue;
        }
        if std::mem::take(&mut skipped) {
            out.push_str("  ...\n");
        }
        let mut line = |sign: &str, side: &Option<(usize, String)>| {
            if let Some((_, text)) = side {
                out.push_str(&format!("{sign}{text}\n"));
            }
        };
        match row.kind {
            RowKind::Same => line("  ", &row.left),
            RowKind::Changed => {
                line("- ", &row.left);
                line("+ ", &row.right);
            }
            RowKind::Removed => line("- ", &row.left),
            RowKind::Added => line("+ ", &row.right),
        }
    }
    if skipped {
        out.push_str("  ...\n");
    }
    out
}

/// Reviewed diffs, keyed by file and request, so a confirmation writes exactly what the human
/// saw even if trust changed the effective values in between.
static REVIEWED: Mutex<Option<HashMap<String, (Instant, String)>>> = Mutex::new(None);

fn review_key(file: &Path, params: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{}\n{params}", file.display()).as_bytes())
    )
}

fn remember_review(key: String, after: &str) -> Result<()> {
    let mut guard = REVIEWED.lock().unwrap_or_else(PoisonError::into_inner);
    let reviewed = guard.get_or_insert_with(HashMap::new);
    reviewed.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(300));
    ensure!(
        reviewed.len() < 32,
        "Too many settings reviews are open; wait five minutes and retry"
    );
    reviewed.insert(key, (Instant::now(), fingerprint(Some(after))));
    Ok(())
}

fn take_review(key: &str) -> Option<String> {
    let mut guard = REVIEWED.lock().unwrap_or_else(PoisonError::into_inner);
    let (at, after) = guard.as_mut()?.remove(key)?;
    (at.elapsed() < Duration::from_secs(300)).then_some(after)
}

/// Replace `file` atomically, only while it still has the reviewed bytes.
fn write(proposal: &Proposal) -> Result<()> {
    let file = &proposal.file;
    let parent = file
        .parent()
        .context("config file has no parent directory")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| anyhow!("Cannot stage the new {} beside it: {error}", file.display()))?;
    match fs::metadata(file) {
        Ok(meta) => staged.as_file().set_permissions(meta.permissions())?,
        #[cfg(unix)]
        Err(_) if proposal.scope == Scope::Project => {
            use std::os::unix::fs::PermissionsExt;
            // A project policy is usually committed; the global one keeps the private default.
            staged
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o644))?;
        }
        Err(_) => {}
    }
    staged.write_all(proposal.after.as_bytes())?;
    staged.as_file().sync_all()?;
    ensure!(
        fingerprint(read_target(file)?.as_deref()) == proposal.base,
        "{} changed while saving; nothing was written. Reload the page and make your edit again",
        file.display()
    );
    let result = if proposal.before.is_none() {
        staged.persist_noclobber(file)
    } else {
        staged.persist(file)
    };
    result.map_err(|error| anyhow!("Cannot replace {}: {}", file.display(), error.error))?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

/// `settings.save {target: "project"|"global", base, values: {key: text}}`.
pub(super) fn save(store: &Store, c: &Console, params: &Value, confirmed: bool) -> Result<Outcome> {
    let loaded = config::load_trusted(store, &c.project, &c.home)?;
    let proposal = propose(&loaded, params)?;
    let file = proposal.file.display().to_string();
    let apply = match proposal.scope {
        Scope::Global => "yoke trust --global".to_owned(),
        Scope::Project => format!(
            "yoke trust from {}",
            proposal.file.parent().unwrap_or(Path::new("/")).display()
        ),
    };
    if proposal.keys.is_empty() {
        return Ok(Outcome::Done(json!({
            "message": "No value differs from the settings in effect; nothing was written."
        })));
    }
    if proposal.before.as_deref() == Some(proposal.after.as_str()) {
        return Ok(Outcome::Done(json!({
            "message": format!("{file} already holds these values; nothing was written. Run `{apply}` in your own terminal to apply them.")
        })));
    }
    let key = review_key(&proposal.file, params);
    if !confirmed {
        let pending = loaded
            .snapshots
            .iter()
            .any(|s| s.file == proposal.file && s.exists && (!s.trusted || s.stale));
        let verb = if proposal.before.is_some() {
            "Change"
        } else {
            "Create"
        };
        let note = if pending {
            "\nThis file also has edits that are not trusted yet; yoke trust shows every pending change before you approve it."
        } else {
            ""
        };
        remember_review(key, &proposal.after)?;
        return Ok(Outcome::Confirm(format!(
            "{verb} {file} ({}):\n\n{}\nNothing is trusted by this save. It applies after you run `{apply}` in your own terminal.{note}",
            proposal.keys.join(", "),
            diff(proposal.before.as_deref().unwrap_or(""), &proposal.after)
        )));
    }
    ensure!(
        take_review(&key) == Some(fingerprint(Some(&proposal.after))),
        "The change differs from the one you reviewed (the settings in effect changed); nothing was written. Review it again"
    );
    write(&proposal)?;
    Ok(Outcome::Done(json!({
        "message": format!("Saved {file}. Nothing changes until you run `{apply}` in your own terminal.")
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use yoke_core::config::{self, Mode};

    fn fixture() -> (tempfile::TempDir, Store, Console) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (home, project) = (root.join("home"), root.join("project"));
        fs::create_dir_all(home.join(".yoke")).unwrap();
        fs::create_dir_all(&project).unwrap();
        let store = Store::open(home.join(".yoke/state.db")).unwrap();
        fs::write(
            home.join(".yoke/config.toml"),
            "[review.models]\nclaude = 'opus'\n",
        )
        .unwrap();
        yoke_core::trust::prepare_global(&store, &home)
            .unwrap()
            .commit(&store)
            .unwrap();
        fs::write(
            project.join("yoke.toml"),
            "# team policy\nmode = 'observe' # start safe\n\n[review]\nreviewer = 'auto'\n",
        )
        .unwrap();
        yoke_core::trust::prepare_project(&store, &project)
            .unwrap()
            .commit(&store)
            .unwrap();
        let c = Console {
            home,
            project,
            database: root.join("unused.db"),
            token: "token".into(),
            csrf: "csrf".into(),
            host: "127.0.0.1:1".into(),
        };
        (temp, store, c)
    }

    fn message(outcome: Outcome) -> String {
        match outcome {
            Outcome::Confirm(message) => message,
            Outcome::Done(data) => data["message"].as_str().unwrap().to_owned(),
        }
    }

    #[test]
    fn save_shows_the_exact_diff_keeps_comments_and_writes_only_after_confirm_without_trusting() {
        let (_temp, store, c) = fixture();
        let file = c.project.join("yoke.toml");
        let before = fs::read_to_string(&file).unwrap();
        let params = json!({"target": "project", "base": page_fingerprint(&file), "values": {
            "mode": "enforce", "review.models.codex": "o3", "stop.max_blocks": "5"}});

        let review = save(&store, &c, &params, false).unwrap();
        assert!(matches!(review, Outcome::Confirm(_)));
        let review = message(review);
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            before,
            "nothing is written before confirming"
        );
        assert!(
            review.contains("- mode = 'observe' # start safe\n+ mode = \"enforce\" # start safe\n"),
            "{review}"
        );
        assert!(review.contains("+ codex = \"o3\"\n"), "{review}");
        assert!(
            !review.contains("max_blocks"),
            "a value equal to the effective one is not written: {review}"
        );
        assert!(review.contains("yoke trust"), "{review}");

        let done = message(save(&store, &c, &params, true).unwrap());
        let after = fs::read_to_string(&file).unwrap();
        assert!(
            after.starts_with("# team policy\nmode = \"enforce\" # start safe\n"),
            "{after}"
        );
        assert!(
            after.contains("[review.models]\ncodex = \"o3\"\n"),
            "{after}"
        );
        assert!(done.contains("yoke trust"), "{done}");
        let loaded = config::load_trusted(&store, &c.project, &c.home).unwrap();
        assert_eq!(loaded.config.mode, Mode::Observe, "a save never trusts");

        let stale = save(&store, &c, &params, true)
            .map(message)
            .unwrap_err()
            .to_string();
        assert!(stale.contains("changed since"), "{stale}");
        assert_eq!(fs::read_to_string(&file).unwrap(), after);
    }

    #[test]
    fn save_refuses_invalid_values_and_edits_another_file_would_override() {
        let (_temp, store, c) = fixture();
        let project = c.project.join("yoke.toml");
        let global = c.home.join(".yoke/config.toml");
        let try_save = |target: &str, file: &Path, values: Value| {
            let params =
                json!({"target": target, "base": page_fingerprint(file), "values": values});
            save(&store, &c, &params, false)
                .map(message)
                .unwrap_err()
                .to_string()
        };
        let error = try_save("project", &project, json!({"stop.max_blocks": "-1"}));
        assert!(
            error.contains("stop.max_blocks must be a whole number"),
            "{error}"
        );
        let error = try_save(
            "project",
            &project,
            json!({"review.models.codex": "--yolo"}),
        );
        assert!(error.contains("review.models.codex"), "{error}");
        let error = try_save("project", &project, json!({"gate": "x"}));
        assert!(error.contains("cannot be edited here"), "{error}");
        let error = try_save("global", &global, json!({"mode": "enforce"}));
        assert!(error.contains("overrides the global file"), "{error}");
        let error = try_save("project", &project, json!({"review.models.claude": ""}));
        assert!(error.contains("nothing to unset"), "{error}");
        assert!(fs::read_to_string(&project)
            .unwrap()
            .starts_with("# team policy"));
    }
}
