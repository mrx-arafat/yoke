use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};

/// Fetch for `yoke update`; errors describe an update download.
pub(super) fn download(url: &str, limit: usize) -> Result<Vec<u8>> {
    fetch("update download", url, limit)
}

/// The same bounded request for `yoke update --check`, which installs nothing,
/// so errors describe a release check rather than a download.
pub(super) fn check(url: &str, limit: usize) -> Result<Vec<u8>> {
    fetch("release check", url, limit)
}

fn fetch(what: &str, url: &str, limit: usize) -> Result<Vec<u8>> {
    ensure!(url.starts_with("https://"), "update requires HTTPS");
    let mut command = Command::new("curl");
    command.args([
        "--disable",
        "--fail",
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
        "--url",
        url,
    ]);
    collect(what, &mut command, limit, Duration::from_secs(35))
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
                ensure!(
                    status.success(),
                    "{what} failed ({status}); check connectivity and release availability"
                );
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
