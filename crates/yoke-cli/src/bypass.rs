//! The ways a human switches Yoke's hooks off, and the home directory hooks trust.
//!
//! An agent's settings files can put environment variables into the hook process, so no
//! environment value alone may skip a hook or choose where Yoke keeps its state:
//! - `YOKE_OFF` skips a hook only when it equals the per-install token in `~/.yoke/off-token`,
//!   which only `yoke pause` prints, and only a human terminal may run that.
//! - `yoke pause --for 30m` skips every hook until a stored expiry instead, with no restart.
//! - A release build's hook ignores a `HOME` that differs from the account's home directory when
//!   that directory already holds Yoke state. Debug builds keep honouring `HOME` so tests can
//!   run in an isolated one.
//!
//! Every honoured or refused skip is written to `error.log`.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use yoke_core::guard::OFF_TOKEN_FILE;
use yoke_core::store::Store;
use yoke_core::timefmt;

use crate::commands::{self, quote};
use crate::hook::append_error;
use crate::install::InstallOptions;

const PAUSE_FILE: &str = "paused-until";
const MAX_PAUSE_SECS: i64 = 24 * 3600;
/// A session that is still rendering its statusline but has had no hook decision this long.
pub const SILENT_SECS: i64 = 20 * 60;
const MAX_SETTINGS_BYTES: u64 = 8 * 1024 * 1024;

pub fn token_path(home: &Path) -> PathBuf {
    home.join(".yoke").join(OFF_TOKEN_FILE)
}

fn pause_path(home: &Path) -> PathBuf {
    home.join(".yoke").join(PAUSE_FILE)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// The stored token, if a well-formed one exists. Never creates it.
fn read_token(home: &Path) -> Option<String> {
    let text = read_control(&token_path(home), 256)?;
    let token = text.trim();
    (token.len() >= 32 && token.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| token.to_owned())
}

/// Bypass controls are owner-only regular files. Invalid controls never disable enforcement.
fn read_control(path: &Path, cap: u64) -> Option<String> {
    if !fs::symlink_metadata(path.parent()?).ok()?.is_dir() {
        return None;
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    // SAFETY: geteuid has no preconditions.
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.len() > cap
    {
        return None;
    }
    let mut text = String::new();
    file.take(cap + 1).read_to_string(&mut text).ok()?;
    (text.len() as u64 <= cap).then_some(text)
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .context("read /dev/urandom for a bypass token")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn private_directory(home: &Path) -> Result<()> {
    yoke_core::private::create_dir_all(&home.join(".yoke"))
        .context("create the Yoke state directory")
}

/// The stored token, created owner-only on first use.
fn ensure_token(home: &Path) -> Result<String> {
    if let Some(token) = read_token(home) {
        return Ok(token);
    }
    private_directory(home)?;
    let path = token_path(home);
    // A leftover that is not a well-formed token is replaced.
    let _ = fs::remove_file(&path);
    let token = random_token()?;
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(format!("{token}\n").as_bytes())
                .context("write the bypass token")?;
            Ok(token)
        }
        // A concurrent `yoke pause` created it first.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            read_token(home).context("read the bypass token another process created")
        }
        Err(error) => Err(error).context("create the bypass token"),
    }
}

fn pause_until(home: &Path) -> Option<i64> {
    read_control(&pause_path(home), 32)?.trim().parse().ok()
}

fn write_pause(home: &Path, until: i64) -> Result<()> {
    private_directory(home)?;
    let path = pause_path(home);
    let staged = path.with_extension("tmp");
    let _ = fs::remove_file(&staged);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staged)
        .and_then(|mut file| file.write_all(format!("{until}\n").as_bytes()))
        .context("write the pause expiry")?;
    fs::rename(&staged, &path).context("store the pause expiry")
}

/// The unexpired pause end, for `yoke health` and the statusline.
pub fn active_pause(home: &Path) -> Option<i64> {
    pause_until(home).filter(|until| *until > now())
}

fn pause_problem(until: i64) -> String {
    format!(
        "Yoke is paused until {}; every hook is skipped and nothing is recorded. Run yoke pause --clear in your own terminal to end the pause",
        timefmt::local_time(until)
    )
}

/// Claude settings files that put `YOKE_OFF` or `HOME` into the environment of every hook, and
/// an active pause: the ways to switch Yoke off that no hook decision records. Read-only.
pub fn switch_problems(
    home: &Path,
    options: Option<&InstallOptions>,
    project: &Path,
) -> Vec<String> {
    let mut problems: Vec<String> = active_pause(home).map(pause_problem).into_iter().collect();
    let Some(options) = options else {
        return problems;
    };
    let mut files = vec![
        options.claude_managed.join("managed-settings.json"),
        options.claude_settings.clone(),
    ];
    for directory in [project, &std::env::current_dir().unwrap_or_default()] {
        for name in ["settings.local.json", "settings.json"] {
            files.push(directory.join(".claude").join(name));
        }
    }
    files.dedup();
    for file in files {
        let Some(settings) = read_settings(&file) else {
            continue;
        };
        for key in ["YOKE_OFF", "HOME"] {
            if settings["env"].get(key).is_some() {
                problems.push(format!(
                    "Claude settings {} set env.{key}, which every hook process inherits; {}. Remove the entry",
                    file.display(),
                    if key == "HOME" {
                        "Yoke's hooks ignore a HOME that differs from your account's home, but it can still redirect other tools"
                    } else {
                        "Yoke skips a hook only when YOKE_OFF equals the private bypass token, which a settings file must never hold"
                    }
                ));
            }
        }
    }
    problems
}

/// A bounded read of one settings file; absent, unreadable and invalid files give `None`.
fn read_settings(path: &Path) -> Option<Value> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SETTINGS_BYTES {
        return None;
    }
    serde_json::from_reader(file).ok()
}

/// What the statusline can see now without the health cache: a pause, a `YOKE_OFF` in this
/// session's own environment, or a session that renders but records no hook decision.
pub fn live_problem(
    home: &Path,
    store: Option<&Store>,
    session: Option<&str>,
    now: i64,
) -> Option<String> {
    if let Some(until) = active_pause(home) {
        return Some(format!(
            "Yoke is paused until {}",
            timefmt::local_time(until)
        ));
    }
    if std::env::var("YOKE_OFF").is_ok_and(|value| !matches!(value.as_str(), "" | "0")) {
        return Some("YOKE_OFF is set in this session's environment; run yoke health".into());
    }
    let silent = store?.session_silence(session?, now).ok().flatten()?;
    (silent >= SILENT_SECS).then(|| {
        format!(
            "no hook decision for {}; hooks may be switched off (run yoke health)",
            timefmt::duration_text(silent)
        )
    })
}

fn same_secret(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0u8, |difference, (left, right)| difference | (left ^ right))
            == 0
}

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// Why the hook is skipped, when it is.
    pub skip: Option<String>,
    /// Why a `YOKE_OFF` request was not honoured, when it was set and wrong.
    pub refused: Option<String>,
}

/// Decide whether a hook is skipped. Reads two small files and nothing else.
pub fn check(home: &Path, off: Option<&str>, now: i64) -> Outcome {
    let mut outcome = Outcome::default();
    if let Some(value) = off.filter(|value| !matches!(*value, "" | "0")) {
        match read_token(home) {
            Some(token) if same_secret(&token, value) => {
                outcome.skip = Some("YOKE_OFF matched the bypass token".into());
                return outcome;
            }
            Some(_) => {
                outcome.refused = Some(
                    "YOKE_OFF is set but does not match the bypass token; the hook ran normally"
                        .into(),
                );
            }
            None => {
                outcome.refused = Some(
                    "YOKE_OFF is set but no bypass token exists; the hook ran normally".into(),
                );
            }
        }
    }
    if let Some(until) = pause_until(home).filter(|until| *until > now) {
        outcome.skip = Some(format!(
            "Yoke is paused until {}",
            timefmt::local_time(until)
        ));
    }
    outcome
}

/// Whether `yoke run-hook` should exit at once: a matching `YOKE_OFF` or an active pause.
/// Reads no stdin, no configuration and no database.
pub fn skips_hook() -> bool {
    let Ok(home) = hook_home() else {
        return false;
    };
    let off = std::env::var("YOKE_OFF").ok();
    let outcome = check(&home, off.as_deref(), now());
    for line in [&outcome.refused, &outcome.skip].into_iter().flatten() {
        let _ = append_error(&home, &format!("bypass: {line}"));
    }
    outcome.skip.is_some()
}

/// Whether a hook honours a `HOME` that differs from the account's home: dev and test builds, and
/// builds made with `--features honor-home-env` for an isolated optimized run. The shipped
/// binary does neither, so a settings file cannot move its state.
const fn honours_home(debug_build: bool, feature: bool) -> bool {
    debug_build || feature
}

/// The home directory a hook process uses for state, trust and logs.
pub fn hook_home() -> Result<PathBuf> {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    let environment = std::env::var_os("HOME").map(PathBuf::from);
    let account = account_home();
    let has_state = account
        .as_ref()
        .is_some_and(|home| home.join(".yoke").is_dir());
    let honour = honours_home(cfg!(debug_assertions), cfg!(feature = "honor-home-env"));
    let (home, ignored) = choose_home(environment, account, has_state, honour)?;
    if let Some(ignored) = ignored {
        if !LOGGED.swap(true, Ordering::SeqCst) {
            let message = format!(
                "bypass: HOME={} differs from the account home; the hook used {}",
                ignored.display(),
                home.display()
            );
            let _ = append_error(&home, &message);
            // An isolated run of an optimized binary lands here too, so say it where it is seen.
            eprintln!(
                "yoke: {message} (an isolated optimized run needs a binary built with --features honor-home-env)"
            );
        }
    }
    Ok(home)
}

/// The home to use and, when one was set aside, the `HOME` that was ignored.
fn choose_home(
    environment: Option<PathBuf>,
    account: Option<PathBuf>,
    account_has_state: bool,
    honour_environment: bool,
) -> Result<(PathBuf, Option<PathBuf>)> {
    // An unset, empty or relative HOME is an error in every build, as for every other command:
    // falling back to the account's home is how an isolated run once reached real state.
    let Some(environment) = environment.filter(|home| home.is_absolute()) else {
        bail!("HOME is not set to an absolute path; Yoke needs it to find its state");
    };
    let Some(account) = account else {
        return Ok((environment, None));
    };
    if environment == account
        || honour_environment
        || !account_has_state
        || same_directory(&environment, &account)
    {
        Ok((environment, None))
    } else {
        Ok((account, Some(environment)))
    }
}

fn same_directory(left: &Path, right: &Path) -> bool {
    matches!((left.canonicalize(), right.canonicalize()), (Ok(left), Ok(right)) if left == right)
}

/// The account's home directory from the user database, not from the environment.
fn account_home() -> Option<PathBuf> {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buffer = vec![0u8; 16 * 1024];
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer names a live local, and the length is the buffer's own.
    let status = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            &mut entry,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        )
    };
    if status != 0 || found.is_null() || entry.pw_dir.is_null() {
        return None;
    }
    // SAFETY: getpwuid_r filled `pw_dir` with a NUL-terminated string inside `buffer`.
    let directory = unsafe { CStr::from_ptr(entry.pw_dir) };
    let path = PathBuf::from(OsStr::from_bytes(directory.to_bytes()));
    path.is_absolute().then_some(path)
}

/// `30m`, `2h` or `90s`, at most 24 hours.
fn parse_duration(text: &str) -> Result<i64> {
    let digits = text.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let unit = &text[digits.len()..];
    let amount: i64 = digits
        .parse()
        .with_context(|| format!("{text:?} is not a duration; use for example 30m, 2h or 90s"))?;
    let seconds = amount
        * match unit {
            "s" => 1,
            "m" => 60,
            "h" => 3600,
            _ => bail!("{text:?} needs a unit: s, m or h (for example 30m)"),
        };
    ensure!(seconds > 0, "a pause must last at least one second");
    ensure!(
        seconds <= MAX_PAUSE_SECS,
        "a pause can last at most 24h; pause again when it ends"
    );
    Ok(seconds)
}

/// `yoke pause`: show the bypass token, or pause every hook for a limited time. Human-only.
pub fn pause(duration: Option<String>, clear: bool) -> Result<()> {
    commands::require_human("pause")?;
    let home = commands::home()?;
    let now = now();
    if clear {
        match fs::remove_file(pause_path(&home)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("remove the pause"),
        }
        let _ = append_error(&home, "bypass: pause cleared by a human");
    } else if let Some(text) = &duration {
        let until = now + parse_duration(text)?;
        write_pause(&home, until)?;
        let _ = append_error(
            &home,
            &format!(
                "bypass: paused by a human until {}",
                timefmt::local_time(until)
            ),
        );
    }
    match active_pause(&home) {
        Some(until) => println!(
            "pause: active until {} ({} left); every hook is skipped and nothing is recorded",
            quote(&timefmt::local_time(until)),
            timefmt::duration_text(until - now)
        ),
        None => println!("pause: not active"),
    }
    if duration.is_none() && !clear {
        println!("token: {}", ensure_token(&home)?);
        println!("help: {}", quote("Keep the token private: whoever sets YOKE_OFF to it can start an agent with Yoke's hooks skipped. In your own terminal run YOKE_OFF=<token> claude or YOKE_OFF=<token> codex, or run yoke pause --for 30m to skip every hook for a limited time without restarting. End a pause with yoke pause --clear. Every skip is written to ~/.yoke/error.log."));
    } else {
        println!(
            "help: {}",
            quote("End a pause early with yoke pause --clear. Run yoke pause alone for the YOKE_OFF token.")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn bypass_controls_reject_links_public_files_and_oversized_content() {
        use std::os::unix::fs::symlink;
        for is_token in [true, false] {
            let home = tempfile::tempdir().unwrap();
            let token = if is_token {
                ensure_token(home.path()).unwrap()
            } else {
                write_pause(home.path(), 2_000).unwrap();
                "not-a-token".into()
            };
            let path = if is_token {
                token_path(home.path())
            } else {
                pause_path(home.path())
            };
            assert!(check(home.path(), Some(&token), 1_000).skip.is_some());
            let state = home.path().join(".yoke");
            let saved = home.path().join("saved-state");
            fs::rename(&state, &saved).unwrap();
            symlink(&saved, &state).unwrap();
            assert_eq!(check(home.path(), Some(&token), 1_000).skip, None);
            fs::remove_file(&state).unwrap();
            fs::rename(&saved, &state).unwrap();
            let target = path.with_extension("saved");
            fs::rename(&path, &target).unwrap();
            symlink(&target, &path).unwrap();
            assert_eq!(check(home.path(), Some(&token), 1_000).skip, None);
            fs::remove_file(&path).unwrap();
            fs::rename(&target, &path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(check(home.path(), Some(&token), 1_000).skip, None);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            let value = if is_token {
                "a".repeat(256)
            } else {
                "2000".into()
            };
            fs::write(&path, format!("{value}{}", " ".repeat(1024))).unwrap();
            assert_eq!(check(home.path(), Some(&value), 1_000).skip, None);
        }
    }

    #[test]
    fn yoke_off_skips_only_with_the_install_token_and_every_refusal_is_reported() {
        let home = tempfile::tempdir().unwrap();
        let token = ensure_token(home.path()).unwrap();
        assert_eq!(ensure_token(home.path()).unwrap(), token);
        let mode = fs::metadata(token_path(home.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);

        let skipped = check(home.path(), Some(&token), 0);
        assert!(skipped.skip.is_some() && skipped.refused.is_none());
        for wrong in ["1", "true", &token[..token.len() - 1]] {
            let refused = check(home.path(), Some(wrong), 0);
            assert!(refused.skip.is_none(), "{wrong}");
            assert!(refused.refused.is_some(), "{wrong}");
        }
        assert_eq!(check(home.path(), Some("0"), 0), Outcome::default());
        assert_eq!(check(home.path(), None, 0), Outcome::default());

        let no_token = tempfile::tempdir().unwrap();
        assert!(check(no_token.path(), Some("1"), 0).refused.is_some());
    }

    #[test]
    fn a_pause_skips_hooks_until_it_ends() {
        let home = tempfile::tempdir().unwrap();
        write_pause(home.path(), 2_000).unwrap();
        assert!(check(home.path(), None, 1_999).skip.is_some());
        assert!(check(home.path(), None, 2_000).skip.is_none());
        let mode = fs::metadata(pause_path(home.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(parse_duration("30m").unwrap(), 1800);
        for bad in ["", "30", "0m", "25h", "m", "-5m", "5d"] {
            assert!(parse_duration(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn home_is_honoured_only_by_dev_builds_and_the_honor_home_env_feature() {
        for (debug_build, feature, honoured) in [
            (false, false, false),
            (true, false, true),
            (false, true, true),
            (true, true, true),
        ] {
            assert_eq!(honours_home(debug_build, feature), honoured);
        }
        // The shipped configuration: feature off, debug assertions off.
        let shipped = honours_home(false, false);
        let (account, injected) = (
            PathBuf::from("/account/home"),
            PathBuf::from("/tmp/isolated"),
        );
        let chosen =
            choose_home(Some(injected.clone()), Some(account.clone()), true, shipped).unwrap();
        assert_eq!(chosen, (account, Some(injected)));
    }

    #[test]
    fn hooks_ignore_a_different_home_only_when_the_account_home_holds_state() {
        let (account, injected) = (
            PathBuf::from("/account/home"),
            PathBuf::from("/tmp/injected"),
        );
        let choose = |environment: Option<&Path>, state, honour| {
            choose_home(
                environment.map(Path::to_path_buf),
                Some(account.clone()),
                state,
                honour,
            )
            .unwrap()
        };
        assert_eq!(
            choose(Some(&injected), true, false),
            (account.clone(), Some(injected.clone()))
        );
        assert_eq!(
            choose(Some(&injected), false, false),
            (injected.clone(), None)
        );
        assert_eq!(
            choose(Some(&injected), true, true),
            (injected.clone(), None)
        );
        assert_eq!(choose(Some(&account), true, false), (account.clone(), None));
        // An unset or relative HOME never falls back to the account's home, in any build.
        for honour in [false, true] {
            for environment in [None, Some(PathBuf::from("relative"))] {
                assert!(choose_home(environment, Some(account.clone()), true, honour).is_err());
            }
        }
        assert!(choose_home(None, None, false, false).is_err());
    }

    #[test]
    fn health_flags_settings_that_inject_yoke_off_or_home_and_an_active_pause() {
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("proj");
        fs::create_dir_all(project.join(".claude")).unwrap();
        fs::write(
            project.join(".claude/settings.local.json"),
            r#"{"env":{"YOKE_OFF":"1","HOME":"/tmp/elsewhere"}}"#,
        )
        .unwrap();
        fs::create_dir_all(home.path().join(".claude")).unwrap();
        fs::write(
            home.path().join(".claude/settings.json"),
            r#"{"env":{"LANG":"C"}}"#,
        )
        .unwrap();
        let mut options =
            crate::install::options(home.path(), "/bin/yoke".into(), Vec::new()).unwrap();
        options.claude_settings = home.path().join(".claude/settings.json");
        options.claude_managed = home.path().join("managed");

        let problems = switch_problems(home.path(), Some(&options), &project);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("env.YOKE_OFF")));
        assert!(problems.iter().any(|p| p.contains("env.HOME")));

        write_pause(home.path(), now() + 600).unwrap();
        let problems = switch_problems(home.path(), Some(&options), &project);
        assert!(
            problems[0].starts_with("Yoke is paused until"),
            "{problems:?}"
        );
        assert!(live_problem(home.path(), None, None, now())
            .unwrap()
            .contains("paused"));
    }

    #[test]
    fn an_open_session_without_a_recent_decision_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let store = Store::open(&path).unwrap();
        let raw = rusqlite::Connection::open(&path).unwrap();
        raw.execute_batch(
            "INSERT INTO sessions(id,agent,project,started_at) VALUES('open','claude','/p',1000);
             INSERT INTO sessions(id,agent,project,started_at,ended_at) VALUES('done','claude','/p',1000,1001);
             INSERT INTO sessions(id,agent,project,started_at) VALUES('busy','codex','/p',1000);
             INSERT INTO decisions(ts,session_id,agent,project,event,mode,outcome,gates,payload)
               VALUES(5000,'busy','codex','/p','stop','observe','allow','[]','{}');",
        )
        .unwrap();
        let now = 5000 + 60;
        assert_eq!(
            store.session_silence("open", now).unwrap(),
            Some(now - 1000)
        );
        assert_eq!(store.session_silence("busy", now).unwrap(), Some(60));
        assert_eq!(store.session_silence("done", now).unwrap(), None);
        assert_eq!(store.session_silence("unknown", now).unwrap(), None);
        let silent = store
            .silent_sessions("/p", now, (SILENT_SECS, 12 * 3600))
            .unwrap();
        assert_eq!(
            silent,
            vec![("open".to_owned(), "claude".to_owned(), now - 1000)]
        );
    }
}
