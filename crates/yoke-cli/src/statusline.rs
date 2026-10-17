//! Read-only status refresh; external wrapping is explicit and bounded.
//!
//! A paint never waits: it reads the agent's callback, one read-only pass over the state
//! database, a few small files, and prints. Anything slow (a git status, a health check) comes
//! from a cache beside the state, refreshed by a detached child, and is left out when absent.
//! The writes are the agent's reported quota and those caches, all in the state directory and
//! only when it already exists.
use std::io::{IsTerminal, Read};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use yoke_core::engine::execute_bounded;
use yoke_core::notifications;
use yoke_core::quota;
use yoke_core::store::Store;
use yoke_core::timefmt;

mod cache;
mod git;
mod line;

use line::{Facts, Look};

const INPUT_CAP: usize = 64 * 1024;
/// Optional facts (git, health) are skipped once a paint has used this long.
const OPTIONAL_BUDGET: Duration = Duration::from_millis(30);
const HEALTH_FILE: &str = "statusline-health.json";
const HEALTH_FRESH: i64 = 60;
const HEALTH_SHOWN: i64 = 3_600;
/// A detached refresh ends itself after this long, whatever it is waiting for.
const CHILD_LIMIT: Duration = Duration::from_secs(5);
/// Columns assumed when the terminal cannot be asked.
const DEFAULT_WIDTH: usize = 120;

/// How much the statusline shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Style {
    /// One line.
    #[default]
    Compact,
    /// The same line plus a second line with the model, last event and both quota resets.
    Detailed,
}

/// How a paint looks and what it may touch; tests build it explicitly.
#[derive(Clone, Debug)]
struct Settings {
    look: Look,
    git: bool,
    health: bool,
}

impl Settings {
    /// `NO_COLOR` (any non-empty value) turns colors off and `YOKE_ASCII=1` limits the line to
    /// 7-bit characters. `YOKE_STATUSLINE_WIDTH` overrides the detected width.
    fn from_env(style: Style) -> Self {
        let var =
            |name: &str| std::env::var_os(name).map(|value| value.to_string_lossy().into_owned());
        Self::from_vars(style, &var, terminal_size)
    }

    /// `from_env` with the variables and the terminal given, so every option is testable.
    fn from_vars(
        style: Style,
        var: &dyn Fn(&str) -> Option<String>,
        terminal: fn() -> Option<usize>,
    ) -> Self {
        let width = ["YOKE_STATUSLINE_WIDTH", "COLUMNS"]
            .into_iter()
            .filter_map(var)
            .find_map(|value| value.trim().parse::<usize>().ok().filter(|n| *n > 0))
            .or_else(terminal);
        Self {
            look: Look {
                color: var("NO_COLOR").is_none_or(|value| value.is_empty()),
                ascii: var("YOKE_ASCII").is_some_and(|value| !value.is_empty() && value != "0"),
                detailed: style == Style::Detailed,
                width: width.unwrap_or(DEFAULT_WIDTH),
            },
            git: true,
            health: true,
        }
    }

    #[cfg(test)]
    fn plain() -> Self {
        Self {
            look: Look {
                color: false,
                ascii: false,
                detailed: false,
                width: 0,
            },
            git: false,
            health: false,
        }
    }
}

/// Columns of the terminal the agent runs in. The callback's stdout is a pipe to the agent, so
/// ask the controlling terminal, then an inherited stream.
fn terminal_size() -> Option<usize> {
    let tty = std::fs::File::open("/dev/tty").ok();
    let descriptors = tty.iter().map(AsRawFd::as_raw_fd).chain([2, 0]);
    for fd in descriptors {
        // SAFETY: `winsize` is plain data and TIOCGWINSZ only writes into it.
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ as _, &mut size) } == 0 && size.ws_col > 0 {
            return Some(usize::from(size.ws_col));
        }
    }
    None
}

fn clean(text: &str, limit: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect()
}

fn context_percentage(payload: &Value) -> Option<f64> {
    let window = &payload["context_window"];
    if let Some(percent) = window["used_percentage"].as_f64() {
        return (percent.is_finite() && (0.0..=100.0).contains(&percent)).then_some(percent);
    }
    let size = window["context_window_size"].as_u64()?;
    if size == 0 {
        return None;
    }
    let usage = &window["current_usage"];
    let used = usage["input_tokens"]
        .as_u64()?
        .checked_add(usage["cache_creation_input_tokens"].as_u64()?)?
        .checked_add(usage["cache_read_input_tokens"].as_u64()?)?;
    (used <= size).then_some(used as f64 * 100.0 / size as f64)
}

/// The line to paint, and a diagnostic for stderr when part of the input could not be used.
struct Rendered {
    /// One line, or two joined by a newline in the detailed style.
    line: String,
    warning: Option<String>,
}

fn unix_now() -> Result<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .try_into()?)
}

fn refresh(home: &Path, input: &[u8], wrap: Option<&str>, settings: &Settings) -> Result<Rendered> {
    let now = unix_now()?;
    refresh_at(
        home,
        input,
        wrap,
        now,
        timefmt::utc_offset_secs(now),
        settings,
    )
}

/// A newer release than this one, from the version the daily check recorded.
fn newer_release(latest: Option<&str>) -> Option<String> {
    let latest = latest?;
    let (latest_version, current) = (
        crate::update::version(latest).ok()?,
        crate::update::version(env!("CARGO_PKG_VERSION")).ok()?,
    );
    (latest_version > current).then(|| latest.to_owned())
}

/// What the callback itself carries: model, cost, duration, lines, context and quota.
fn from_payload(facts: &mut Facts, payload: &Value, home: &Path, now: i64) {
    facts.model = payload["model"]["display_name"]
        .as_str()
        .or_else(|| payload["model"]["id"].as_str())
        .map(str::to_owned);
    let cost = &payload["cost"];
    facts.cost_usd = cost["total_cost_usd"].as_f64();
    facts.duration_ms = cost["total_duration_ms"].as_u64();
    let (added, removed) = (
        cost["total_lines_added"].as_u64(),
        cost["total_lines_removed"].as_u64(),
    );
    facts.lines =
        (added.is_some() || removed.is_some()).then(|| (added.unwrap_or(0), removed.unwrap_or(0)));
    facts.context = context_percentage(payload);
    if let Some(reading) = quota::Quota::from_claude(payload, now) {
        // The line shows what this callback carries. The record feeds `yoke usage`, which has no
        // callback of its own, so a failed write only leaves it showing an older observed time.
        let _ = quota::record_file(&quota::claude_file(home), &reading, now);
        facts.quota = reading.windows;
    }
}

/// Yoke's own facts, from one read-only pass over the state database. The snapshot is required;
/// the rest degrade to absent, with the first failure reported on stderr rather than in the bar.
fn from_state(
    facts: &mut Facts,
    store: &Store,
    (session, project, dir): (Option<&str>, Option<&str>, &Path),
    now: i64,
) -> Result<Option<String>> {
    let snapshot = store.status_snapshot(session, project, now)?;
    facts.mode = snapshot.mode;
    facts.plan = snapshot.plan_slug.zip(snapshot.plan_state);
    facts.recent_deny = snapshot.recent_deny;
    facts.update = newer_release(snapshot.latest_version.as_deref());
    let mut failure = None;
    let mut note = |what: &str, error: anyhow::Error| {
        failure.get_or_insert_with(|| format!("{what}: {error:#}"));
    };
    // Session ids identify the project even when the status callback runs in a subdirectory.
    let recorded = match session.map(|id| store.session_project(id)).transpose() {
        Ok(recorded) => recorded.flatten(),
        Err(error) => {
            note("session lookup failed", error);
            None
        }
    };
    let known_session = recorded.is_some();
    let project = recorded.as_deref().or(project);
    // A session no hook has recorded yet still belongs to the project that contains its directory.
    match store.policy_trusted(project.or(dir.to_str())) {
        Ok(trusted) => facts.trusted = Some(trusted),
        Err(error) => note("trust lookup failed", error),
    }
    if let Some(session) = session.filter(|_| known_session) {
        match store.session_counts(session) {
            Ok(counts) => facts.counts = Some(counts),
            Err(error) => note("decision counts failed", error),
        }
    }
    // An older schema has no notifications table; the line paints without the field then.
    facts.unread = notifications::unread_for_status(store, session, project).unwrap_or(0);
    Ok(failure)
}

/// The directory the session works in, which is where git is asked and never anywhere else.
fn working_dir(payload: &Value, fallback: &Path) -> PathBuf {
    [&payload["workspace"]["current_dir"], &payload["cwd"]]
        .into_iter()
        .filter_map(Value::as_str)
        .map(PathBuf::from)
        .find(|dir| dir.is_absolute())
        .unwrap_or_else(|| fallback.to_path_buf())
}

/// The first problem `yoke health` would report, as the last background check found it. A stale
/// or missing answer starts a detached refresh and is not waited for.
fn cached_problem(home: &Path, dir: &Path, now: i64) -> Option<String> {
    let key = dir.to_string_lossy();
    let entry = cache::load(home, HEALTH_FILE, &key);
    let age = entry.as_ref().and_then(|entry| entry.age(now));
    let busy = entry.as_ref().is_some_and(|entry| entry.in_flight(now));
    let shown = age
        .filter(|age| *age <= HEALTH_SHOWN)
        .and(entry.as_ref())
        .and_then(|entry| entry.value["problem"].as_str().map(str::to_owned));
    if age.is_none_or(|age| age > HEALTH_FRESH) && !busy {
        let _ = cache::refresh(home, HEALTH_FILE, &key, entry, now, "--refresh-health", dir);
    }
    shown
}

/// `refresh` with the clock and time zone given, so the time-dependent fields are testable.
fn refresh_at(
    home: &Path,
    input: &[u8],
    wrap: Option<&str>,
    now: i64,
    offset_secs: i64,
    settings: &Settings,
) -> Result<Rendered> {
    let started = Instant::now();
    ensure!(input.len() <= INPUT_CAP, "statusline input exceeds 64 KiB");
    // The agent repaints this line constantly, so input Yoke cannot read still gets the line
    // from recorded state, and the reason goes to stderr instead of into the prompt bar.
    let (payload, mut warning) = if input.iter().all(u8::is_ascii_whitespace) {
        (Value::Null, None)
    } else {
        match serde_json::from_slice(input) {
            Ok(payload) => (payload, None),
            Err(error) => (
                Value::Null,
                Some(format!(
                    "invalid statusline JSON: {error}; showing recorded state only"
                )),
            ),
        }
    };
    let db = home.join(".yoke/state.db");
    let store = match std::fs::metadata(&db) {
        Ok(_) => {
            let store = Store::open_read_only(&db).context("read statusline database")?;
            // A file with no committed schema (e.g. zero bytes) has no state yet, like a missing one.
            (store.schema_version()?.0 > 0).then_some(store)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let cwd = std::env::current_dir()?;
    let session = payload["session_id"].as_str();
    let project = if session.is_some() {
        None
    } else {
        payload["workspace"]["project_dir"]
            .as_str()
            .or_else(|| payload["cwd"].as_str())
            .or_else(|| cwd.to_str())
    };
    let dir = working_dir(&payload, &cwd);
    let mut facts = Facts::default();
    from_payload(&mut facts, &payload, home, now);
    facts.session = session.map(str::to_owned);
    if let Some(store) = &store {
        let failure = from_state(&mut facts, store, (session, project, &dir), now)?;
        warning = warning.or(failure);
    }
    if settings.git && started.elapsed() < OPTIONAL_BUDGET {
        facts.branch = git::status(home, &dir, now);
    }
    if settings.health && store.is_some() && started.elapsed() < OPTIONAL_BUDGET {
        facts.problem = cached_problem(home, &dir, now);
    }
    // A pause, a YOKE_OFF in this environment or a silent session outrank the cached check.
    if let Some(problem) = crate::bypass::live_problem(home, store.as_ref(), session, now) {
        facts.problem = Some(problem);
    }
    let wrapped = wrap.map(|wrap| run_wrapper(wrap, input)).transpose()?;
    let mut look = settings.look.clone();
    if let Some(wrapped) = wrapped.as_ref().filter(|wrapped| !wrapped.is_empty()) {
        // Reserve room for the wrapped command's output so the line as a whole still fits;
        // width 0 means no limit and stays that way.
        if look.width > 0 {
            look.width = look
                .width
                .saturating_sub(wrapped.chars().count() + 3)
                .max(1);
        }
    }
    let mut lines = line::render(&facts, &look, now, offset_secs);
    if let Some(wrapped) = wrapped.filter(|wrapped| !wrapped.is_empty()) {
        lines[0].push_str(" | ");
        lines[0].push_str(&wrapped);
    }
    Ok(Rendered {
        line: lines.join("\n"),
        warning,
    })
}

/// Run the user's own statusline command with the callback's input, under a hard limit.
fn run_wrapper(wrap: &str, input: &[u8]) -> Result<String> {
    ensure!(
        std::env::var_os("YOKE_STATUSLINE_WRAPPED").is_none(),
        "recursive statusline wrapper refused"
    );
    ensure!(
        !wrap.trim().is_empty() && wrap.len() <= 4096 && !wrap.contains('\0'),
        "invalid statusline wrapper"
    );
    let output = execute_bounded(
        Command::new("/bin/sh")
            .arg("-c")
            .arg(wrap)
            .env("YOKE_OFF", "1")
            .env("YOKE_STATUSLINE_WRAPPED", "1"),
        input,
        Duration::from_millis(250),
        4096,
    )?;
    ensure!(
        output.success && !output.timed_out,
        "statusline wrapper failed or exceeded 250 ms"
    );
    ensure!(
        !output.stdout_truncated,
        "statusline wrapper output exceeds 4096 bytes"
    );
    Ok(clean(&String::from_utf8_lossy(&output.stdout), 512)
        .trim()
        .to_owned())
}

fn render_stdin(wrap: Option<&str>, style: Style) -> Result<Rendered> {
    let mut input = Vec::new();
    if !std::io::stdin().is_terminal() {
        std::io::stdin()
            .take((INPUT_CAP + 1) as u64)
            .read_to_end(&mut input)?;
    }
    refresh(
        &crate::commands::home()?,
        &input,
        wrap,
        &Settings::from_env(style),
    )
}

/// The detached children `cache::refresh` starts: no input, no output, one fact each.
fn refresh_child(git_dir: Option<&Path>, health_dir: Option<&Path>) -> Result<()> {
    // A refresh that hangs, on a stalled filesystem for instance, must not outlive the guard
    // that lets the next paint start another one.
    std::thread::spawn(|| {
        std::thread::sleep(CHILD_LIMIT);
        std::process::exit(0)
    });
    let home = crate::commands::home()?;
    let now = unix_now()?;
    if let Some(dir) = git_dir {
        git::refresh(&home, dir, now)?;
    }
    if let Some(dir) = health_dir {
        let problem = crate::health::first_problem()?;
        cache::save(
            &home,
            HEALTH_FILE,
            &dir.to_string_lossy(),
            cache::Entry {
                at: now,
                started: 0,
                value: json!({ "problem": problem }),
            },
        )?;
    }
    Ok(())
}

/// Consume the callback's EOF-terminated input, read existing state, print one line (two in the
/// detailed style). The agent paints stdout in its prompt bar, so diagnostics go to stderr only,
/// and a failure prints nothing there rather than the usual `error:` and `help:` block.
pub fn run(
    wrap: Option<&str>,
    style: Style,
    refresh_git: Option<&Path>,
    refresh_health: Option<&Path>,
) -> Result<()> {
    if refresh_git.is_some() || refresh_health.is_some() {
        return refresh_child(refresh_git, refresh_health);
    }
    match render_stdin(wrap, style) {
        Ok(rendered) => {
            println!("{}", rendered.line);
            if let Some(warning) = rendered.warning {
                eprintln!("yoke: {warning}");
            }
            Ok(())
        }
        Err(error) => {
            eprintln!("yoke: {error:#}");
            std::process::exit(1)
        }
    }
}

#[cfg(test)]
mod tests;
