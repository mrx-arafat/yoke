use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};

/// Fetch a release file for `yoke update`; errors describe an update download.
pub(super) fn download(url: &str, limit: usize) -> Result<Vec<u8>> {
    fetch("update download", url, limit)
}

/// The release metadata request shared by `yoke update` and `yoke update --check`,
/// which installs nothing, so its errors describe a lookup rather than a download.
pub(super) fn lookup(url: &str, limit: usize) -> Result<Vec<u8>> {
    fetch("release lookup", url, limit)
}

/// What curl prints after the body: a newline and the three-digit HTTP status.
const STATUS_TRAILER: usize = 4;

/// A GitHub answer that was not a success; `update.rs` treats 404 on the lookup as "no release".
#[derive(Debug)]
pub(super) struct HttpStatus {
    what: String,
    code: u16,
}

impl HttpStatus {
    pub(super) fn new(what: &str, code: u16) -> Self {
        Self {
            what: what.into(),
            code,
        }
    }

    pub(super) fn code(&self) -> u16 {
        self.code
    }
}

impl std::fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = &self.what;
        match self.code {
            404 => write!(f, "{what} failed: GitHub answered 404, the file was not found"),
            403 | 429 => write!(
                f,
                "{what} failed: GitHub rate limit reached (HTTP {}); try again later, unauthenticated requests are limited per IP address",
                self.code
            ),
            500..=599 => write!(
                f,
                "{what} failed: GitHub answered HTTP {}; try again later",
                self.code
            ),
            code => write!(f, "{what} failed: GitHub answered HTTP {code}"),
        }
    }
}

impl std::error::Error for HttpStatus {}

/// Split the trailing HTTP status off curl's output and keep the body only for a 2xx answer.
pub(super) fn split_status(what: &str, mut bytes: Vec<u8>, limit: usize) -> Result<Vec<u8>> {
    ensure!(
        bytes.len() >= STATUS_TRAILER,
        "{what} failed: no HTTP status in the response"
    );
    let body_len = bytes.len() - STATUS_TRAILER;
    let code = std::str::from_utf8(&bytes[body_len..])
        .ok()
        .and_then(|trailer| trailer.strip_prefix('\n'))
        .and_then(|digits| digits.parse::<u16>().ok())
        .with_context(|| format!("{what} failed: no HTTP status in the response"))?;
    if !(200..300).contains(&code) {
        return Err(HttpStatus::new(what, code).into());
    }
    bytes.truncate(body_len);
    ensure!(bytes.len() <= limit, "{what} exceeds size limit");
    Ok(bytes)
}

/// Why curl itself failed, in words about the user's situation rather than curl's numbers.
fn exit_message(what: &str, status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(6) => format!("{what} failed (curl exit 6): cannot resolve the GitHub host; check DNS and connectivity"),
        Some(7) => format!("{what} failed (curl exit 7): cannot connect to GitHub; check connectivity"),
        Some(28) => format!("{what} failed (curl exit 28): timed out; check connectivity"),
        Some(35 | 60) => format!("{what} failed (curl exit {}): the TLS connection to GitHub failed; check the system clock and certificates", status.code().unwrap_or_default()),
        Some(63) => format!("{what} exceeds size limit"),
        _ => format!("{what} failed ({status}); check connectivity"),
    }
}

fn fetch(what: &str, url: &str, limit: usize) -> Result<Vec<u8>> {
    ensure!(url.starts_with("https://"), "update requires HTTPS");
    let mut command = Command::new("curl");
    // No `--fail`: its exit 22 hides the HTTP status. `--write-out` appends the status instead.
    command.args([
        "--disable",
        "--silent",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--max-redirs",
        "3",
        "--connect-timeout",
        "5",
        "--max-time",
        "30",
        "--max-filesize",
        &limit.to_string(),
        "--header",
        "Accept: application/vnd.github+json",
        "--user-agent",
        concat!("yoke/", env!("CARGO_PKG_VERSION")),
        "--write-out",
        "\n%{http_code}",
        "--url",
        url,
    ]);
    let bytes = collect(
        what,
        &mut command,
        limit + STATUS_TRAILER,
        Duration::from_secs(35),
    )?;
    split_status(what, bytes, limit)
}

/// Run `command` with bounded output and time; `what` names the operation in errors.
pub(super) fn collect(
    what: &str,
    command: &mut Command,
    limit: usize,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {what}; ensure curl is installed"))?;
    let stdout = child
        .stdout
        .take()
        .with_context(|| format!("{what} stdout unavailable"))?;
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(result);
    });
    let start = Instant::now();
    let result = (|| {
        let bytes = receiver
            .recv_timeout(timeout)
            .with_context(|| format!("{what} exceeded deadline"))??;
        ensure!(bytes.len() <= limit, "{what} exceeds size limit");
        loop {
            if let Some(status) = child.try_wait()? {
                ensure!(status.success(), "{}", exit_message(what, status));
                return Ok(bytes);
            }
            if start.elapsed() >= timeout {
                bail!("{what} exceeded deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() {
        // Reap curl even when its own timeout did not fire.
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}
