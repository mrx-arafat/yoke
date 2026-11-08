//! Bounded Git runs for console features: hardened command, no shell, hard timeout.
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

const TIMEOUT: Duration = Duration::from_secs(30);
/// Diffs and listings beyond this are cut off and marked, never held whole in memory.
const OUTPUT_LIMIT: u64 = 4 * 1024 * 1024;

pub(crate) struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    /// Standard output reached the size limit and was cut off.
    pub truncated: bool,
}

/// Run `git -C <dir> <args>` with an argument array. Nothing is parsed by a shell.
pub(crate) fn run(dir: &Path, args: &[&str]) -> Result<Output> {
    let mut command = crate::process::git()?;
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("starting git")?;
    let mut out = child.stdout.take().context("git stdout")?;
    let mut err = child.stderr.take().context("git stderr")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = (&mut out).take(OUTPUT_LIMIT + 1).read_to_end(&mut bytes);
        // Drain the rest so git is never blocked on a full pipe while we wait for it.
        let _ = std::io::copy(&mut out, &mut std::io::sink());
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = (&mut err).take(64 * 1024).read_to_end(&mut bytes);
        let _ = std::io::copy(&mut err, &mut std::io::sink());
        bytes
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "git {} did not finish within {} seconds and was stopped; check the repository and retry",
                args.first().copied().unwrap_or(""),
                TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut stdout = reader.join().unwrap_or_default();
    let truncated = stdout.len() as u64 > OUTPUT_LIMIT;
    stdout.truncate(OUTPUT_LIMIT as usize);
    Ok(Output {
        success: status.success(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_reader.join().unwrap_or_default())
            .trim()
            .to_owned(),
        truncated,
    })
}

/// Like `run`, but a non-zero exit becomes an error carrying Git's own message.
pub(crate) fn run_ok(dir: &Path, args: &[&str]) -> Result<String> {
    let output = run(dir, args)?;
    if !output.success {
        bail!(
            "git {} failed: {}",
            args.first().copied().unwrap_or(""),
            if output.stderr.is_empty() {
                "no message"
            } else {
                output.stderr.as_str()
            }
        );
    }
    Ok(output.stdout)
}
