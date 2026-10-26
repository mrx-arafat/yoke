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
        }?;
        write!(f, "; retry yoke update --check after resolving the response; the installed executable has not been replaced")
    }
}

impl std::error::Error for HttpStatus {}

/// Split the trailing HTTP status off curl's output and keep the body only for a 2xx answer.
pub(super) fn split_status(what: &str, mut bytes: Vec<u8>, limit: usize) -> Result<Vec<u8>> {
    ensure!(
        bytes.len() >= STATUS_TRAILER,
        "{what} failed: no HTTP status in the response; check connectivity and retry yoke update --check"
    );
    let body_len = bytes.len() - STATUS_TRAILER;
    let code = std::str::from_utf8(&bytes[body_len..])
        .ok()
        .and_then(|trailer| trailer.strip_prefix('\n'))
        .and_then(|digits| digits.parse::<u16>().ok())
        .with_context(|| format!("{what} failed: no HTTP status in the response; check connectivity and retry yoke update --check"))?;
    if !(200..300).contains(&code) {
        return Err(HttpStatus::new(what, code).into());
    }
    bytes.truncate(body_len);
    ensure!(bytes.len() <= limit, "{what} exceeds size limit; keep the installed executable and retry yoke update --check after the release asset is corrected");
    Ok(bytes)
}

/// Why curl itself failed, in words about the user's situation rather than curl's numbers.
fn exit_message(what: &str, status: std::process::ExitStatus) -> String {
    let reason = match status.code() {
        Some(6) => format!("{what} failed (curl exit 6): cannot resolve the GitHub host; check DNS and connectivity"),
        Some(7) => format!("{what} failed (curl exit 7): cannot connect to GitHub; check connectivity"),
        Some(28) => format!("{what} failed (curl exit 28): timed out; check connectivity"),
        Some(35 | 60) => format!("{what} failed (curl exit {}): the TLS connection to GitHub failed; check the system clock and certificates", status.code().unwrap_or_default()),
        Some(63) => format!("{what} exceeds size limit"),
        _ => format!("{what} failed ({status}); check connectivity"),
    };
    format!("{reason}; retry yoke update --check after resolving the cause; the installed executable has not been replaced")
}

/// Where curl is looked for before falling back to the `PATH` lookup, so a `curl` that a
/// project or shell startup file put early on `PATH` is not run with the user's rights.
pub(super) const CURL_PATHS: [&str; 4] = [
    "/usr/bin/curl",
    "/bin/curl",
    "/usr/local/bin/curl",
    "/opt/homebrew/bin/curl",
];

/// The first of `candidates` that is a file, else `curl` for the `PATH` lookup.
pub(super) fn curl_program(candidates: &[&str]) -> std::path::PathBuf {
    candidates
        .iter()
        .map(std::path::PathBuf::from)
        .find(|path| path.is_file())
        .unwrap_or_else(|| "curl".into())
}

/// Whether `YOKE_CURL` may name the curl to run: dev and test builds, and builds made with
/// `--features honor-home-env`, so tests can fake the transport. The shipped binary ignores it,
/// so a settings file cannot swap the program that downloads the executable hooks run.
pub(super) const fn honours_curl_override(debug_build: bool, feature: bool) -> bool {
    debug_build || feature
}

/// `requested` (the `YOKE_CURL` value) when overrides are honoured and it is not empty, else the
/// first fixed location that exists.
pub(super) fn select_curl(
    candidates: &[&str],
    requested: Option<std::ffi::OsString>,
    honoured: bool,
) -> std::path::PathBuf {
    match requested.filter(|path| honoured && !path.is_empty()) {
        Some(path) => path.into(),
        None => curl_program(candidates),
    }
}

pub(super) fn curl_command(program: &std::path::Path, url: &str, limit: usize) -> Command {
    let mut command = Command::new(program);
    // No `--fail`: its exit 22 hides the HTTP status. `--write-out` appends the status instead.
    command.args([
        "--disable",
        "--silent",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--tlsv1.2",
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
    command
}

fn fetch(what: &str, url: &str, limit: usize) -> Result<Vec<u8>> {
    ensure!(url.starts_with("https://"), "update requires HTTPS");
    let program = select_curl(
        &CURL_PATHS,
        std::env::var_os("YOKE_CURL"),
        honours_curl_override(cfg!(debug_assertions), cfg!(feature = "honor-home-env")),
    );
    let mut command = curl_command(&program, url, limit);
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
        let bytes = receiver.recv_timeout(timeout).with_context(|| {
            format!("{what} exceeded deadline; check connectivity and retry yoke update --check")
        })??;
        ensure!(bytes.len() <= limit, "{what} exceeds size limit; keep the installed executable and retry yoke update --check after the release asset is corrected");
        loop {
            if let Some(status) = child.try_wait()? {
                ensure!(status.success(), "{}", exit_message(what, status));
                return Ok(bytes);
            }
            if start.elapsed() >= timeout {
                bail!("{what} exceeded deadline; check connectivity and retry yoke update --check");
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
