//! System: logs, background jobs, backups, retention, memory export and health.
//! Each section is a server-rendered view (`system?view=<name>`). Everything that changes or
//! deletes data asks the human to confirm first, and nothing here restores a database: restore
//! is a human-only terminal command (`yoke backup restore`).
use super::{actions::Outcome, render, BadRequest, Console};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::sync::{Mutex, PoisonError};
use yoke_core::store::Store;

#[path = "system/backups.rs"]
mod backups;
#[path = "system/export.rs"]
mod export;
#[path = "system/health.rs"]
mod health;
#[path = "system/jobs.rs"]
mod jobs;
#[path = "system/logs.rs"]
mod logs;
#[path = "system/retention.rs"]
mod retention;

/// The sections in tab order; the first is the default.
const VIEWS: [(&str, &str); 6] = [
    ("health", "Health"),
    ("logs", "Logs"),
    ("jobs", "Jobs"),
    ("backups", "Backups"),
    ("retention", "Retention"),
    ("export", "Export"),
];

pub(super) fn page(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let view = params.get("view").map_or(VIEWS[0].0, String::as_str);
    if !VIEWS.iter().any(|(name, _)| *name == view) {
        return Err(BadRequest("Choose a System section from its tabs.".into()).into());
    }
    let mut out = render::heading(
        "system",
        "Logs, background jobs, backups, retention, memory export and health for this machine's Yoke state.",
    );
    out.push_str(&tabs(view));
    out.push_str(&match view {
        "jobs" => jobs::section(store, params)?,
        "backups" => backups::section(c, params)?,
        "retention" => retention::section(store, c)?,
        "export" => export::section(c),
        "logs" => logs::section(store, c, params)?,
        _ => health::section(c),
    });
    out.push_str("</main>");
    Ok(out)
}

fn tabs(current: &str) -> String {
    let links: String = VIEWS
        .iter()
        .map(|(name, label)| {
            format!(
                r#"<a data-nav href="system?view={name}"{}>{label}</a>"#,
                if *name == current {
                    r#" aria-current="page""#
                } else {
                    ""
                }
            )
        })
        .collect();
    format!(r#"<nav class="sys-tabs" aria-label="System sections">{links}</nav>"#)
}

/// Handle a delegated console action. Return `Outcome::Confirm(message)` for a step that needs
/// the human to confirm; the action is then re-run with `confirmed = true` after they do.
pub(super) fn act(
    store: &Store,
    c: &Console,
    name: &str,
    params: &Value,
    confirmed: bool,
) -> Result<Outcome> {
    match name {
        "system.logs-clear" => logs::clear(c, params, confirmed),
        "system.backup-create" => backups::create(store, c),
        "system.backup-delete" => backups::delete(c, params, confirmed),
        "system.retention-run" => retention::run(store, c, confirmed, &mut || {
            crate::jobs::spawn_worker(&c.home)
        }),
        "system.job-retry" => jobs::retry(store, c, params, confirmed, &mut || {
            crate::jobs::spawn_worker(&c.home)
        }),
        _ => bail!("Unknown action"),
    }
}

/// A file the console sends as a download, from GET `system/download?backup=<name>` or
/// `system/download?memory=project|all`.
pub(super) struct Download {
    file: File,
    length: u64,
    /// File name offered to the browser; always plain ASCII.
    name: String,
    kind: &'static str,
    /// A generated file lives in a private temporary folder until the download is sent.
    _temporary: Option<tempfile::TempDir>,
}

/// Choose the download a GET `system/download` request names.
pub(super) fn download(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<Download> {
    match (params.get("backup"), params.get("memory"), params.len()) {
        (Some(name), None, 1) => backups::download(c, name),
        (None, Some(scope), 1) => export::download(store, c, scope),
        _ => Err(BadRequest("Choose a backup or an export from System.".into()).into()),
    }
}

impl Download {
    /// Stream the file with the console's security headers and an attachment disposition.
    pub(super) fn send(mut self, request: super::http::Request) -> Result<()> {
        let mut headers = super::security_headers(self.kind);
        headers.push((
            "Content-Disposition".into(),
            format!("attachment; filename=\"{}\"", self.name),
        ));
        request.respond_file(200, headers, &mut self.file, self.length)?;
        Ok(())
    }
}

/// Unix seconds now; a clock before 1970 reads as 0.
pub(super) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// An integer parameter of a delegated action.
fn int(params: &Value, key: &str) -> Result<i64> {
    params
        .get(key)
        .and_then(Value::as_i64)
        .with_context(|| format!("Missing {key}"))
}

/// `512 bytes`, `3.4 KiB`, `81.2 MiB` or `1.1 GiB`.
fn size_text(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = "KiB";
    for next in ["MiB", "GiB"] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next;
    }
    format!("{value:.1} {unit}")
}

/// `'path'` for a POSIX shell.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Errors this console process hit since it started, newest last; bounded and in memory only.
static CONSOLE_ERRORS: Mutex<VecDeque<(i64, String)>> = Mutex::new(VecDeque::new());
const CONSOLE_ERROR_CAP: usize = 100;

/// Keep a redacted copy of a console error for the Logs view. The console calls this where it
/// prints a request error, so the human can see failures without the terminal.
pub(super) fn record_console_error(message: &str) {
    let mut ring = CONSOLE_ERRORS
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if ring.len() == CONSOLE_ERROR_CAP {
        ring.pop_front();
    }
    ring.push_back((now(), yoke_core::redact::redact(message)));
}

fn console_errors() -> Vec<(i64, String)> {
    CONSOLE_ERRORS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .cloned()
        .collect()
}

fn clear_console_errors() -> usize {
    let mut ring = CONSOLE_ERRORS
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let count = ring.len();
    ring.clear();
    count
}

#[cfg(test)]
fn test_console(root: &std::path::Path) -> Console {
    let (home, project) = (root.join("home"), root.join("app"));
    for dir in [home.join(".yoke"), project.clone()] {
        std::fs::create_dir_all(dir).unwrap();
    }
    Console {
        home,
        project,
        database: root.join("home/.yoke/state.db"),
        token: "token".into(),
        csrf: "csrf".into(),
        host: "127.0.0.1:1".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn a_download_streams_past_the_response_cap_as_an_attachment_with_security_headers() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("big.db");
        let size = 16 * 1024 * 1024 + 1;
        std::fs::write(&path, vec![7_u8; size]).unwrap();
        let download = Download {
            file: File::open(&path).unwrap(),
            length: size as u64,
            name: "state-1.db".into(),
            kind: "application/vnd.sqlite3",
            _temporary: None,
        };
        let server = super::super::http::Server::bind_loopback().unwrap();
        let mut client = std::net::TcpStream::connect(server.local_addr().unwrap()).unwrap();
        client
            .write_all(b"GET /system/download HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let request = server.recv().unwrap();
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            client.read_to_end(&mut bytes).unwrap();
            bytes
        });
        download.send(request).unwrap();
        let bytes = reader.join().unwrap();
        let split = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&bytes[..split]);
        assert!(head.contains("Content-Disposition: attachment; filename=\"state-1.db\""));
        assert!(head.contains(&format!("Content-Length: {size}")));
        assert!(
            head.contains("X-Content-Type-Options: nosniff")
                && head.contains("Cache-Control: no-store")
        );
        assert_eq!(bytes.len() - split - 4, size);
    }
}
