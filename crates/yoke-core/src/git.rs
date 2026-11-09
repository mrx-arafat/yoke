//! The one Git helper. Every call is an argument array to the real `git` (never a shell
//! string), with fsmonitor off and a deadline, and nothing is ever forced.
//!
//! - [`Repo`] serves the console's Changes page: status, diffs, staging, commits, branches,
//!   remotes and stashes. Repository hooks run, as for a commit typed in a terminal, and only
//!   environment that would aim git at another repository is removed.
//! - [`run`] and [`run_ok`] serve everything else (plan worktrees, PRD recovery, the team
//!   remote): hooks disabled, every `GIT_*` variable removed, no optional locks, 30 seconds,
//!   and the first 4 MiB of standard output.
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};

use crate::engine::{execute_bounded, ProcessOutput};

mod commit_message;
mod diff;
mod log;
mod remote;
pub use commit_message::{CommitDraft, StagedDiff};
pub use diff::{FileDiff, Hunk, Line, LineKind};
pub use log::CommitTime;
pub use remote::{Branch, PullStrategy, PushPlan, Stash};

const READ: Duration = Duration::from_secs(30);
const LOCAL: Duration = Duration::from_secs(120);
const COMMIT: Duration = Duration::from_secs(300);
const NETWORK: Duration = Duration::from_secs(120);
const OUTPUT_CAP: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 5000;

/// Environment that would aim git at a different repository than the one asked for.
const REPOSITORY_OVERRIDES: [&str; 9] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_LITERAL_PATHSPECS",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
}

impl Kind {
    fn from_letter(letter: char) -> Option<Self> {
        Some(match letter {
            'M' => Self::Modified,
            'A' => Self::Added,
            'D' => Self::Deleted,
            'R' => Self::Renamed,
            'C' => Self::Copied,
            'T' => Self::TypeChanged,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::Added => "added",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Copied => "copied",
            Self::TypeChanged => "type changed",
        }
    }
}

/// One changed path. A file with both staged and unstaged edits is one entry in both groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    /// The previous path of a staged or unstaged rename or copy.
    pub orig_path: Option<String>,
    pub staged: Option<Kind>,
    pub unstaged: Option<Kind>,
    pub untracked: bool,
    pub conflicted: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// `None` while HEAD is detached.
    pub branch: Option<String>,
    /// Abbreviated HEAD commit; `None` before the first commit.
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub entries: Vec<Entry>,
    /// More than the listed number of paths changed.
    pub truncated: bool,
    /// A merge, rebase, cherry-pick, revert or bisect that a human must finish or abort.
    pub operation: Option<&'static str>,
}

impl Status {
    /// Tracked files are unchanged; untracked files do not count.
    pub fn tracked_clean(&self) -> bool {
        self.entries
            .iter()
            .all(|entry| entry.untracked && entry.staged.is_none() && !entry.conflicted)
    }

    fn entry(&self, path: &str) -> Result<&Entry> {
        self.entries
            .iter()
            .find(|entry| entry.path == path)
            .with_context(|| {
                format!("{path} has no changes in this repository; refresh the Changes page")
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Staged,
    Unstaged,
    Untracked,
}

impl Side {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "staged" => Some(Self::Staged),
            "unstaged" => Some(Self::Unstaged),
            "untracked" => Some(Self::Untracked),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Unstaged => "unstaged",
            Self::Untracked => "untracked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    pub hash: String,
    pub subject: String,
}

#[derive(Debug, Clone)]
pub struct Repo {
    root: PathBuf,
}

fn base_command(dir: &Path, read_only: bool) -> Result<Command> {
    let mut command = Command::new(crate::process::executable("git")?);
    for name in REPOSITORY_OVERRIDES {
        command.env_remove(name);
    }
    command
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .env("GIT_PAGER", "cat")
        .args(["-c", "core.fsmonitor=false"]);
    if read_only {
        command.arg("--no-optional-locks");
    }
    Ok(command)
}

/// The command names the git subcommand a message is about.
fn verb<'a>(args: &[&'a str]) -> &'a str {
    args.iter()
        .copied()
        .find(|arg| !arg.starts_with('-'))
        .unwrap_or("git")
}

fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    let start = text.len().saturating_sub(2000);
    let start = (start..=text.len())
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or(text.len());
    crate::redact::redact(&text[start..])
}

/// A path git reports, safe to hand back to git: relative, inside the repository, no NUL.
fn relative(path: &str) -> Result<&str> {
    ensure!(
        !path.is_empty()
            && path.len() <= 4096
            && !path.contains('\0')
            && Path::new(path)
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "path {path:?} is not a path inside the repository"
    );
    Ok(path)
}

/// A pathspec that matches this exact file name, whatever characters it contains. Applied per
/// path rather than with `--literal-pathspecs`, which changes how `git stash` reads `stash@{n}`.
fn literal(path: &str) -> String {
    format!(":(literal){path}")
}

/// A branch or start-point name that git accepts and that cannot be read as an option.
fn ref_name(name: &str) -> Result<&str> {
    ensure!(
        !name.is_empty() && !name.starts_with('-') && name.len() <= 255,
        "{name:?} is not a usable branch name"
    );
    Ok(name)
}

impl Repo {
    /// The work tree containing `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let mut command = base_command(path, true)?;
        command.args(["rev-parse", "--show-toplevel"]);
        let output = execute_bounded(&mut command, &[], READ, OUTPUT_CAP)
            .context("run git; install it and add its directory to PATH")?;
        ensure!(
            output.success,
            "{} is not inside a Git work tree; the Changes page needs a project that is a Git repository",
            path.display()
        );
        let root = String::from_utf8(output.stdout).context("repository path is not UTF-8")?;
        Ok(Self {
            root: PathBuf::from(root.trim_end_matches('\n')).canonicalize()?,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn command(&self, read_only: bool, network: bool) -> Result<Command> {
        let mut command = base_command(&self.root, read_only)?;
        if network && !self.ssh_customised() {
            // No tty prompt can be answered from a browser, so fail fast instead of hanging.
            command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
        }
        Ok(command)
    }

    fn ssh_customised(&self) -> bool {
        if std::env::var_os("GIT_SSH_COMMAND").is_some() || std::env::var_os("GIT_SSH").is_some() {
            return true;
        }
        let Ok(mut command) = base_command(&self.root, true) else {
            return true;
        };
        command.args(["config", "--get", "core.sshCommand"]);
        execute_bounded(&mut command, &[], READ, 4096)
            .map(|output| output.success && !output.stdout.is_empty())
            .unwrap_or(true)
    }

    fn execute(
        &self,
        read_only: bool,
        network: bool,
        args: &[&str],
        input: &[u8],
        timeout: Duration,
        cap: usize,
    ) -> Result<ProcessOutput> {
        let mut command = self.command(read_only, network)?;
        command.args(args);
        let verb = verb(args);
        let output = execute_bounded(&mut command, input, timeout, cap)
            .with_context(|| format!("run git {verb}"))?;
        ensure!(
            !output.timed_out,
            "git {verb} timed out after {} seconds{}",
            timeout.as_secs(),
            if network {
                "; Yoke never asks for a password, so sign in to the remote once in a terminal (an SSH agent or a Git credential helper), then retry"
            } else {
                "; check that no hook or editor is waiting for input, then retry"
            }
        );
        Ok(output)
    }

    fn run_with(
        &self,
        read_only: bool,
        network: bool,
        args: &[&str],
        input: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let output = self.execute(read_only, network, args, input, timeout, OUTPUT_CAP)?;
        let verb = verb(args);
        if !output.success {
            let mut detail = tail(&output.stderr);
            if detail.is_empty() {
                detail = tail(&output.stdout);
            }
            bail!("git {verb} failed: {detail}");
        }
        ensure!(
            !output.stdout_truncated,
            "git {verb} produced more than {} MiB of output",
            OUTPUT_CAP / 1024 / 1024
        );
        Ok(output.stdout)
    }

    fn read(&self, args: &[&str]) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.run_with(true, false, args, &[], READ)?).into_owned())
    }

    fn write(&self, args: &[&str]) -> Result<String> {
        let output = self.run_with(false, false, args, &[], LOCAL)?;
        Ok(String::from_utf8_lossy(&output).trim().to_owned())
    }

    fn online(&self, args: &[&str]) -> Result<String> {
        let output = self.execute(false, true, args, &[], NETWORK, OUTPUT_CAP)?;
        let combined = [output.stderr.as_slice(), output.stdout.as_slice()].concat();
        if !output.success {
            bail!("git {} failed: {}", verb(args), tail(&combined));
        }
        Ok(tail(&combined))
    }

    /// Current branch, tracking counts, changed paths and any unfinished operation.
    pub fn status(&self) -> Result<Status> {
        let bytes = self.run_with(
            true,
            false,
            &[
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=all",
            ],
            &[],
            READ,
        )?;
        let mut status = parse_status(&bytes);
        let git_dir = self.read(&["rev-parse", "--absolute-git-dir"])?;
        status.operation = operation(Path::new(git_dir.trim_end_matches('\n')));
        Ok(status)
    }

    /// The diff of one changed file on one side. `path` must be a path `status` reports.
    pub fn file_diff(&self, path: &str, side: Side) -> Result<FileDiff> {
        let path = relative(path)?;
        let status = self.status()?;
        let entry = status.entry(path)?;
        let present = match side {
            Side::Staged => entry.staged.is_some(),
            Side::Unstaged => entry.unstaged.is_some() || entry.conflicted,
            Side::Untracked => entry.untracked,
        };
        ensure!(
            present,
            "{path} has no {} changes; refresh the Changes page",
            side.as_str()
        );
        let common = ["diff", "--no-color", "--no-ext-diff", "--no-textconv"];
        let text = match side {
            Side::Untracked => {
                let meta = std::fs::symlink_metadata(self.root.join(path))?;
                if !meta.is_file() && !meta.file_type().is_symlink() {
                    return Ok(FileDiff::note(
                        path,
                        "Not a regular file, so there is no text to show.",
                    ));
                }
                if meta.len() > diff::MAX_BYTES as u64 {
                    return Ok(FileDiff::note(
                        path,
                        &format!(
                            "{} KiB is too large to display here; open the file in your editor.",
                            meta.len() / 1024
                        ),
                    ));
                }
                let mut args = common.to_vec();
                args.extend(["--no-index", "--", "/dev/null", path]);
                self.diff_text(&args, true)?
            }
            Side::Staged => {
                let specs: Vec<String> = std::iter::once(path)
                    .chain(entry.orig_path.as_deref())
                    .map(literal)
                    .collect();
                let mut args = common.to_vec();
                args.extend(["--cached", "--find-renames", "--"]);
                args.extend(specs.iter().map(String::as_str));
                self.diff_text(&args, false)?
            }
            Side::Unstaged => {
                let spec = literal(path);
                let mut args = common.to_vec();
                args.extend(["--", &spec]);
                self.diff_text(&args, false)?
            }
        };
        Ok(match text {
            Some(text) => diff::parse(path, &text),
            None => FileDiff::note(path, "The diff is larger than the console shows."),
        })
    }

    /// `None` when the output exceeded the cap. `--no-index` exits 1 when files differ.
    fn diff_text(&self, args: &[&str], no_index: bool) -> Result<Option<String>> {
        let output = self.execute(true, false, args, &[], READ, diff::MAX_BYTES)?;
        let ok = output.success || (no_index && output.exit_code == Some(1));
        ensure!(ok, "git diff failed: {}", tail(&output.stderr));
        if output.stdout_truncated {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
    }

    fn known<'a>(
        &self,
        status: &'a Status,
        paths: &[String],
        keep: impl Fn(&Entry) -> bool,
    ) -> Result<Vec<&'a Entry>> {
        ensure!(!paths.is_empty(), "choose at least one file");
        ensure!(paths.len() <= 2000, "choose at most 2000 files at once");
        paths
            .iter()
            .map(|path| {
                let entry = status.entry(relative(path)?)?;
                ensure!(
                    keep(entry),
                    "{path} has nothing to do here; refresh the Changes page"
                );
                Ok(entry)
            })
            .collect()
    }

    pub fn stage(&self, paths: &[String]) -> Result<()> {
        let status = self.status()?;
        let entries = self.known(&status, paths, |entry| {
            entry.unstaged.is_some() || entry.untracked || entry.conflicted
        })?;
        let specs: Vec<String> = entries.iter().map(|entry| literal(&entry.path)).collect();
        let mut args = vec!["add", "-A", "--"];
        args.extend(specs.iter().map(String::as_str));
        self.write(&args).map(drop)
    }

    pub fn unstage(&self, paths: &[String]) -> Result<()> {
        let status = self.status()?;
        let entries = self.known(&status, paths, |entry| entry.staged.is_some())?;
        // A staged rename is two index changes; unstaging only the new path would leave a delete.
        let specs: Vec<String> = entries
            .iter()
            .flat_map(|entry| {
                std::iter::once(entry.path.as_str()).chain(entry.orig_path.as_deref())
            })
            .map(literal)
            .collect();
        let mut args = vec!["reset", "-q", "--"];
        args.extend(specs.iter().map(String::as_str));
        self.write(&args).map(drop)
    }

    pub fn stage_all(&self) -> Result<()> {
        self.write(&["add", "-A"]).map(drop)
    }

    pub fn unstage_all(&self) -> Result<()> {
        self.write(&["reset", "-q"]).map(drop)
    }

    /// Commit what is staged. Repository hooks run, as they would for a commit from a terminal.
    pub fn commit(&self, subject: &str, body: &str) -> Result<Committed> {
        let subject = subject.trim();
        let body = body.trim();
        ensure!(!subject.is_empty(), "write a commit message first");
        ensure!(
            !subject.contains(['\n', '\0']) && subject.chars().count() <= 500,
            "the first line must be a single line of at most 500 characters; put details in the description"
        );
        ensure!(
            !body.contains('\0') && body.len() <= 100_000,
            "the description is too long"
        );
        let status = self.status()?;
        ensure!(
            !status.entries.iter().any(|entry| entry.conflicted),
            "resolve the conflicted files and stage them before committing"
        );
        ensure!(
            status.entries.iter().any(|entry| entry.staged.is_some()),
            "nothing is staged; stage files first"
        );
        let message = if body.is_empty() {
            format!("{subject}\n")
        } else {
            format!("{subject}\n\n{body}\n")
        };
        self.run_with(
            false,
            false,
            &["commit", "--cleanup=whitespace", "-F", "-"],
            message.as_bytes(),
            COMMIT,
        )?;
        Ok(Committed {
            hash: self.write(&["rev-parse", "--short", "HEAD"])?,
            subject: subject.to_owned(),
        })
    }
}

/// Deadline of one [`run`].
const RUN_TIMEOUT: Duration = Duration::from_secs(30);
/// Output of one [`run`] beyond this is cut off and marked, never held whole in memory.
const RUN_OUTPUT_LIMIT: u64 = 4 * 1024 * 1024;

pub(crate) struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    /// Standard output reached the size limit and was cut off.
    pub truncated: bool,
}

/// Run `git -C <dir> <args>` with hooks disabled (see [`crate::process::git`]). Nothing is
/// parsed by a shell.
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
        let _ = (&mut out)
            .take(RUN_OUTPUT_LIMIT + 1)
            .read_to_end(&mut bytes);
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
        if started.elapsed() > RUN_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "git {} did not finish within {} seconds and was stopped; check the repository and retry",
                args.first().copied().unwrap_or(""),
                RUN_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut stdout = reader.join().unwrap_or_default();
    let truncated = stdout.len() as u64 > RUN_OUTPUT_LIMIT;
    stdout.truncate(RUN_OUTPUT_LIMIT as usize);
    Ok(Output {
        success: status.success(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_reader.join().unwrap_or_default())
            .trim()
            .to_owned(),
        truncated,
    })
}

/// Like [`run`], but a non-zero exit becomes an error carrying Git's own message.
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

fn parse_status(bytes: &[u8]) -> Status {
    let mut status = Status::default();
    let mut tokens = bytes.split(|byte| *byte == 0);
    while let Some(raw) = tokens.next() {
        let line = String::from_utf8_lossy(raw);
        let Some(kind) = line.chars().next() else {
            continue;
        };
        let (fields, orig) = match kind {
            '#' => {
                header(&mut status, &line[1..]);
                continue;
            }
            '1' => (line.splitn(9, ' ').collect::<Vec<_>>(), None),
            '2' => (
                line.splitn(10, ' ').collect::<Vec<_>>(),
                tokens
                    .next()
                    .map(|orig| String::from_utf8_lossy(orig).into_owned()),
            ),
            'u' => (line.splitn(11, ' ').collect::<Vec<_>>(), None),
            '?' => {
                push(&mut status, untracked(&line[line.len().min(2)..]));
                continue;
            }
            _ => continue,
        };
        let (Some(xy), Some(path)) = (fields.get(1), fields.last()) else {
            continue;
        };
        let mut letters = xy.chars();
        let (index, work) = (letters.next().unwrap_or('.'), letters.next().unwrap_or('.'));
        push(
            &mut status,
            Entry {
                path: (*path).to_owned(),
                orig_path: orig,
                staged: Kind::from_letter(index),
                unstaged: Kind::from_letter(work),
                untracked: false,
                conflicted: kind == 'u',
            },
        );
    }
    status.entries.sort_by(|a, b| a.path.cmp(&b.path));
    status
}

fn untracked(path: &str) -> Entry {
    Entry {
        path: path.to_owned(),
        orig_path: None,
        staged: None,
        unstaged: None,
        untracked: true,
        conflicted: false,
    }
}

fn push(status: &mut Status, entry: Entry) {
    if status.entries.len() >= MAX_ENTRIES {
        status.truncated = true;
    } else {
        status.entries.push(entry);
    }
}

fn header(status: &mut Status, line: &str) {
    let Some((name, value)) = line.trim().split_once(' ') else {
        return;
    };
    match name {
        "branch.head" if value != "(detached)" => status.branch = Some(value.to_owned()),
        "branch.oid" if value != "(initial)" => {
            status.head = Some(value.chars().take(7).collect());
        }
        "branch.upstream" => status.upstream = Some(value.to_owned()),
        "branch.ab" => {
            let mut counts = value
                .split(' ')
                .filter_map(|part| part[1..].parse::<u32>().ok());
            status.ahead = counts.next().unwrap_or(0);
            status.behind = counts.next().unwrap_or(0);
        }
        _ => {}
    }
}

fn operation(git_dir: &Path) -> Option<&'static str> {
    [
        ("MERGE_HEAD", "merge"),
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
        ("BISECT_LOG", "bisect"),
    ]
    .into_iter()
    .find(|(name, _)| git_dir.join(name).exists())
    .map(|(_, label)| label)
}

#[cfg(test)]
mod tests;
