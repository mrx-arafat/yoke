//! Opt-in team remote for extensions: a Git repository the human names, kept as a Yoke-owned
//! clone under the state directory.
//!
//! The address lives in `team-remote.json` beside the state database, never in trusted policy.
//! Yoke stores no credentials and never prompts: Git runs without a terminal, so the remote has
//! to accept an SSH key or a credential helper the human already set up. Nothing is forced.
//! Pulling only moves the clone to the remote's tip; installing and pushing are separate steps
//! that the caller confirms first. Everything read from the clone is treated as untrusted:
//! links and special files are refused and sizes are capped.
//!
//! Layout (the same as Pilot's default): `skills/<name>/SKILL.md` (plus the skill's own files),
//! `rules/<name>.md`, `commands/<name>.md`, `agents/<name>.md`, and for Codex
//! `codex/skills/<name>/SKILL.md`, `codex/rules/<name>.rules`, `codex/agents/<name>.toml`.

mod lines;

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::extension_files::{self as files, Target};
use crate::extensions::{Kind, Owner, Roots, Scope};
use crate::git::Repo;
use crate::gitcmd;
use crate::redact::redact;

pub use lines::{side_by_side, Row, RowKind};

const LAYOUT: [(&str, Owner, Kind); 7] = [
    ("skills", Owner::Claude, Kind::Skill),
    ("rules", Owner::Claude, Kind::Rule),
    ("commands", Owner::Claude, Kind::Command),
    ("agents", Owner::Claude, Kind::Agent),
    ("codex/skills", Owner::Codex, Kind::Skill),
    ("codex/rules", Owner::Codex, Kind::Rule),
    ("codex/agents", Owner::Codex, Kind::Agent),
];
/// Largest file compared or shown; larger ones are marked, not read.
const FILE_CAP: u64 = 1024 * 1024;
const MAX_FILES: usize = 500;
const MAX_DEPTH: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

pub fn state_file(state_dir: &Path) -> PathBuf {
    state_dir.join("team-remote.json")
}

/// The Yoke-owned clone; the only directory this module ever deletes.
pub fn clone_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("team-remote").join("repo")
}

pub fn load(state_dir: &Path) -> Result<Option<State>> {
    match fs::read_to_string(state_file(state_dir)) {
        Ok(text) => Ok(Some(
            serde_json::from_str(&text).context("team-remote.json is damaged; connect again")?,
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn save(state_dir: &Path, state: &State) -> Result<()> {
    fs::create_dir_all(state_dir)?;
    let file = state_file(state_dir);
    let temp = file.with_extension("json.tmp");
    fs::write(&temp, serde_json::to_vec_pretty(state)?)?;
    fs::rename(&temp, &file)?;
    Ok(())
}

/// A repository address Yoke is willing to hand to Git: https, ssh, git, file, an absolute path
/// or `user@host:path`, with no option-like start, no `ext::` style transport, no whitespace
/// and no password or token in it.
pub fn check_url(url: &str) -> Result<String> {
    let url = url.trim();
    ensure!(
        !url.is_empty() && url.len() <= 2048,
        "Enter the Git repository address"
    );
    ensure!(
        !url.starts_with('-')
            && !url.contains("::")
            && !url.chars().any(|c| c.is_control() || c.is_whitespace()),
        "That address cannot be used"
    );
    let form = "Use an https://, ssh:// or user@host:path address";
    if let Some((scheme, rest)) = url.split_once("://") {
        ensure!(matches!(scheme, "https" | "ssh" | "git" | "file"), "{form}");
        let authority = rest.split('/').next().unwrap_or_default();
        ensure!(
            authority
                .rsplit_once('@')
                .is_none_or(|(userinfo, _)| !userinfo.contains(':')),
            "Do not put a password or token in the address; Yoke never stores credentials. Sign in with an SSH key or a Git credential helper instead"
        );
    } else if !url.starts_with('/') {
        let scp = url.split_once('@').is_some_and(|(user, rest)| {
            !user.is_empty()
                && rest
                    .split_once(':')
                    .is_some_and(|(host, path)| !host.is_empty() && !path.is_empty())
        });
        ensure!(scp, "{form}");
    }
    Ok(url.to_owned())
}

fn check_branch(branch: &str) -> Result<String> {
    let branch = branch.trim();
    ensure!(
        !branch.is_empty()
            && branch.len() <= 255
            && !branch.starts_with(['-', '/'])
            && !branch.ends_with(['/', '.'])
            && !branch.contains("..")
            && !branch.ends_with(".lock")
            && branch
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/')),
        "That branch name cannot be used"
    );
    Ok(branch.to_owned())
}

/// Save the address and fetch it once to prove it is reachable; on failure nothing is kept.
pub fn connect(state_dir: &Path, url: &str, branch: Option<&str>) -> Result<Synced> {
    let state = State {
        url: check_url(url)?,
        branch: branch
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(check_branch)
            .transpose()?,
    };
    discard_clone(state_dir)?;
    save(state_dir, &state)?;
    pull(state_dir).inspect_err(|_| {
        let _ = disconnect(state_dir);
    })
}

/// Forget the address and delete the clone. Extensions already installed stay where they are.
pub fn disconnect(state_dir: &Path) -> Result<()> {
    match fs::remove_file(state_file(state_dir)) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    discard_clone(state_dir)
}

fn discard_clone(state_dir: &Path) -> Result<()> {
    let dir = clone_dir(state_dir);
    match fs::symlink_metadata(&dir) {
        Ok(meta) => {
            ensure!(meta.is_dir(), "{} is not a folder", dir.display());
            fs::remove_dir_all(&dir)?;
            if let Some(parent) = dir.parent() {
                let _ = fs::remove_dir(parent);
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn open_clone(state_dir: &Path, state: &State) -> Result<(Repo, PathBuf)> {
    let dir = clone_dir(state_dir);
    if !dir.join(".git").exists() {
        fs::create_dir_all(&dir)?;
        gitcmd::run_ok(&dir, &["init", "--quiet"])?;
        gitcmd::run_ok(&dir, &["remote", "add", "origin", &state.url])?;
    }
    let repo = Repo::open(&dir)?;
    // A damaged clone folder must never resolve to some enclosing repository.
    ensure!(
        repo.root() == dir.canonicalize()?,
        "The team clone is not a repository of its own; disconnect and connect again"
    );
    gitcmd::run_ok(&dir, &["remote", "set-url", "origin", &state.url])?;
    Ok((repo, dir))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Synced {
    pub branch: String,
    /// The remote has no commits yet; the first push creates the branch.
    pub empty: bool,
}

/// Fetch and move the clone to the remote branch's tip. The only network step besides a push.
pub fn pull(state_dir: &Path) -> Result<Synced> {
    let state = load(state_dir)?.context("No team remote is connected")?;
    let (repo, dir) = open_clone(state_dir, &state)?;
    repo.fetch()?;
    let listed = gitcmd::run_ok(
        &dir,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/remotes/origin",
        ],
    )?;
    let branches: Vec<String> = listed
        .lines()
        .filter_map(|line| line.strip_prefix("origin/"))
        .map(str::to_owned)
        .collect();
    if branches.is_empty() {
        let branch = state.branch.clone().unwrap_or_else(|| "main".into());
        gitcmd::run_ok(
            &dir,
            &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
        )?;
        return Ok(Synced {
            branch,
            empty: true,
        });
    }
    let branch = match &state.branch {
        Some(wanted) => {
            ensure!(
                branches.contains(wanted),
                "The remote has no branch {wanted}; it has {}",
                branches.join(", ")
            );
            wanted.clone()
        }
        None => ["main", "master"]
            .iter()
            .find(|name| branches.iter().any(|b| b == *name))
            .map(|name| (*name).to_owned())
            .or_else(|| (branches.len() == 1).then(|| branches[0].clone()))
            .with_context(|| {
                format!(
                    "The remote has several branches ({}); connect again and name the one to use",
                    branches.join(", ")
                )
            })?,
    };
    gitcmd::run_ok(
        &dir,
        &[
            "checkout",
            "--quiet",
            "-B",
            &branch,
            "--track",
            &format!("origin/{branch}"),
        ],
    )?;
    Ok(Synced {
        branch,
        empty: false,
    })
}

/// An extension in the clone.
#[derive(Clone, Debug)]
pub struct Remote {
    pub agent: Owner,
    pub kind: Kind,
    pub name: String,
    /// The file, or a skill's folder, inside the clone.
    pub unit: PathBuf,
    /// A skill's files relative to its folder; empty for a single-file extension.
    pub files: Vec<String>,
}

fn agent_str(agent: Owner) -> &'static str {
    match agent {
        Owner::Claude => "claude",
        Owner::Codex => "codex",
        Owner::Yoke => "yoke",
    }
}

fn kind_str(kind: Kind) -> &'static str {
    match kind {
        Kind::Skill => "skill",
        Kind::Rule => "rule",
        Kind::Command => "command",
        Kind::Agent => "agent",
        _ => "other",
    }
}

fn id_of(agent: Owner, kind: Kind, name: &str) -> String {
    format!("{}/{}/{name}", agent_str(agent), kind_str(kind))
}

impl Remote {
    /// `claude/skill/review`: stable across pulls and safe to put in a link.
    pub fn id(&self) -> String {
        id_of(self.agent, self.kind, &self.name)
    }
}

#[derive(Debug, Default)]
pub struct Listing {
    pub items: Vec<Remote>,
    /// Entries left out, each with the reason.
    pub skipped: Vec<String>,
}

/// What the clone holds after the last pull. Reads only local files.
pub fn list(state_dir: &Path) -> Result<Listing> {
    let dir = clone_dir(state_dir);
    let mut listing = Listing::default();
    if !dir.is_dir() {
        return Ok(listing);
    }
    for (relative, agent, kind) in LAYOUT {
        let folder = dir.join(relative);
        let Ok(read) = fs::read_dir(&folder) else {
            continue;
        };
        let mut entries: Vec<_> = read.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let shown = format!("{relative}/{file_name}");
            let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                listing.skipped.push(format!("{shown}: a link"));
                continue;
            }
            let (name, files) = if kind == Kind::Skill {
                if !meta.is_dir() || !entry.path().join("SKILL.md").is_file() {
                    continue;
                }
                match walk_files(&entry.path()) {
                    Ok(files) => (file_name.clone(), files),
                    Err(error) => {
                        listing.skipped.push(format!("{shown}: {error:#}"));
                        continue;
                    }
                }
            } else {
                let ext = files::extension(agent, kind).unwrap_or("md");
                let Some(stem) = file_name.strip_suffix(&format!(".{ext}")) else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                (stem.to_owned(), Vec::new())
            };
            if !files::valid_name(&name) {
                listing
                    .skipped
                    .push(format!("{shown}: the name is not usable here"));
                continue;
            }
            listing.items.push(Remote {
                agent,
                kind,
                name,
                unit: entry.path(),
                files,
            });
        }
    }
    Ok(listing)
}

/// Regular files below `dir` as `a/b` paths; a link or special file stops the walk.
fn walk_files(dir: &Path) -> Result<Vec<String>> {
    let mut found = Vec::new();
    let mut pending = vec![(dir.to_path_buf(), String::new(), 0usize)];
    while let Some((path, prefix, depth)) = pending.pop() {
        ensure!(depth <= MAX_DEPTH, "folders are nested too deeply");
        let mut entries: Vec<_> = fs::read_dir(&path)?.collect::<std::io::Result<_>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow!("a file name is not valid text"))?;
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.is_file() {
                found.push(relative);
                ensure!(found.len() <= MAX_FILES, "more than {MAX_FILES} files");
            } else if meta.is_dir() {
                if name != ".git" {
                    pending.push((entry.path(), relative, depth + 1));
                }
            } else {
                bail!("{relative} is a link or special file");
            }
        }
    }
    found.sort();
    Ok(found)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileState {
    Same,
    Differs,
    LocalOnly,
    RemoteOnly,
}

#[derive(Debug)]
pub struct FileCompare {
    pub path: String,
    pub state: FileState,
    /// Text of each side when it is shown; `None` for a missing, binary or very large file.
    pub local: Option<String>,
    pub remote: Option<String>,
    pub note: Option<&'static str>,
}

enum Bytes {
    Missing,
    Data(Vec<u8>),
    Large,
}

fn read_bytes(path: &Path) -> Result<Bytes> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Bytes::Missing),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        meta.is_file(),
        "{} is a link or special file",
        path.display()
    );
    if meta.len() > FILE_CAP {
        return Ok(Bytes::Large);
    }
    Ok(Bytes::Data(fs::read(path)?))
}

/// Compare a remote extension with the local one at `local_unit` (a missing one counts as all
/// remote-only). Local and remote must be plain files and folders.
pub fn compare(remote: &Remote, local_unit: Option<&Path>) -> Result<Vec<FileCompare>> {
    let single = remote.kind != Kind::Skill;
    let mut paths = remote.files.clone();
    if !single {
        if let Some(local) = local_unit.filter(|unit| unit.is_dir()) {
            paths.extend(walk_files(local)?);
        }
        paths.sort();
        paths.dedup();
    } else {
        paths = vec![remote
            .unit
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()];
    }
    let mut out = Vec::new();
    for path in paths {
        let (remote_path, local_path) = if single {
            (remote.unit.clone(), local_unit.map(Path::to_path_buf))
        } else {
            (
                remote.unit.join(&path),
                local_unit.map(|unit| unit.join(&path)),
            )
        };
        let remote_bytes = read_bytes(&remote_path)?;
        let local_bytes = match &local_path {
            Some(path) => read_bytes(path)?,
            None => Bytes::Missing,
        };
        let text = |bytes: &Bytes| match bytes {
            Bytes::Data(data) => String::from_utf8(data.clone()).ok(),
            _ => None,
        };
        let (state, note) = match (&local_bytes, &remote_bytes) {
            (Bytes::Missing, Bytes::Missing) => continue,
            (Bytes::Missing, _) => (FileState::RemoteOnly, None),
            (_, Bytes::Missing) => (FileState::LocalOnly, None),
            (Bytes::Data(a), Bytes::Data(b)) if a == b => (FileState::Same, None),
            (Bytes::Data(_), Bytes::Data(_)) => (FileState::Differs, None),
            _ => (FileState::Differs, Some("too large to compare here")),
        };
        let note = note.or_else(|| {
            (matches!(&local_bytes, Bytes::Data(_)) && text(&local_bytes).is_none()
                || matches!(&remote_bytes, Bytes::Data(_)) && text(&remote_bytes).is_none())
            .then_some("binary file")
        });
        out.push(FileCompare {
            path,
            state,
            local: text(&local_bytes),
            remote: text(&remote_bytes),
            note,
        });
    }
    Ok(out)
}

#[derive(Debug)]
pub struct InstallPlan {
    pub from: PathBuf,
    pub to: PathBuf,
    /// A local copy exists; it is moved to the trash first.
    pub replaces: bool,
    pub files: Vec<String>,
}

pub fn plan_install(roots: &Roots, remote: &Remote, scope: Scope) -> Result<InstallPlan> {
    ensure!(
        matches!(scope, Scope::User | Scope::Project),
        "Pick user or project scope"
    );
    let to = files::unit_path(roots, remote.agent, remote.kind, scope, &remote.name).with_context(
        || {
            format!(
                "A {} cannot be installed in the {} scope for {}",
                kind_str(remote.kind),
                files::scope_label(scope),
                agent_str(remote.agent)
            )
        },
    )?;
    let root = files::kind_root(roots, remote.agent, remote.kind, scope)
        .context("No folder for this extension")?;
    files::ensure_no_links(&root, &to)?;
    let replaces = fs::symlink_metadata(&to).is_ok();
    let files = if remote.files.is_empty() {
        vec![remote
            .unit
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()]
    } else {
        remote.files.clone()
    };
    Ok(InstallPlan {
        from: remote.unit.clone(),
        to,
        replaces,
        files,
    })
}

/// Install; an existing local copy goes to the trash first. Returns the trash folder used.
pub fn apply_install(plan: &InstallPlan, state_dir: &Path, stamp: u64) -> Result<Option<PathBuf>> {
    let mut trashed = None;
    if plan.replaces {
        let change = files::plan_trash(&plan.to, "replaced", state_dir, stamp)?;
        files::apply(&change)?;
        trashed = Some(change.to);
    }
    files::install_copy(&plan.from, &plan.to).map_err(|error| match &trashed {
        Some(trash) => error.context(format!(
            "The previous copy is in the trash at {}",
            trash.display()
        )),
        None => error,
    })?;
    Ok(trashed)
}

/// A local extension picked for sharing.
#[derive(Clone, Debug)]
pub struct Shared {
    pub agent: Owner,
    pub kind: Kind,
    pub name: String,
    pub unit: PathBuf,
}

impl Shared {
    pub fn id(&self) -> String {
        id_of(self.agent, self.kind, &self.name)
    }

    /// Path of this extension inside the clone, relative to its root.
    fn repo_path(&self) -> Result<String> {
        let folder = LAYOUT
            .iter()
            .find(|(_, agent, kind)| *agent == self.agent && *kind == self.kind)
            .map(|(folder, ..)| *folder)
            .with_context(|| {
                format!(
                    "{} is not shared for {}",
                    kind_str(self.kind),
                    agent_str(self.agent)
                )
            })?;
        Ok(match files::extension(self.agent, self.kind) {
            Some(ext) if self.kind != Kind::Skill => format!("{folder}/{}.{ext}", self.name),
            _ => format!("{folder}/{}", self.name),
        })
    }
}

/// A local extension that can be shared: a plain top-level skill, rule, command or agent.
pub fn shared_from(target: &Target) -> Result<Shared> {
    target.ensure_plain()?;
    ensure!(
        target.is_top_level(),
        "Only extensions directly in their folder are shared, not ones in subfolders"
    );
    let name = match target.kind {
        Kind::Skill => target.unit.file_name(),
        _ => target.unit.file_stem(),
    }
    .map(|name| name.to_string_lossy().into_owned())
    .unwrap_or_default();
    ensure!(
        files::valid_name(&name),
        "'{name}' is not a name the team remote can hold"
    );
    let shared = Shared {
        agent: target.agent,
        kind: target.kind,
        name,
        unit: target.unit.clone(),
    };
    shared.repo_path()?;
    Ok(shared)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Add,
    Modify,
    Remove,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// Path in the repository.
    pub path: String,
    pub kind: ChangeKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharePlan {
    /// The remote address with any credentials stripped and secret-shaped text masked.
    pub url: String,
    pub branch: String,
    pub changes: Vec<FileChange>,
}

/// Per file: (path in the repository, local file, file in the clone).
fn file_pairs(item: &Shared, clone: &Path) -> Result<Vec<(String, PathBuf, PathBuf)>> {
    let base = item.repo_path()?;
    let remote_unit = clone.join(&base);
    if item.kind != Kind::Skill {
        return Ok(vec![(base, item.unit.clone(), remote_unit)]);
    }
    let mut names = walk_files(&item.unit)?;
    if remote_unit.is_dir() {
        names.extend(walk_files(&remote_unit)?);
    }
    names.sort();
    names.dedup();
    Ok(names
        .into_iter()
        .map(|name| {
            (
                format!("{base}/{name}"),
                item.unit.join(&name),
                remote_unit.join(&name),
            )
        })
        .collect())
}

/// What pushing `items` would change in the clone as last pulled.
pub fn plan_push(state_dir: &Path, items: &[Shared]) -> Result<SharePlan> {
    ensure!(!items.is_empty(), "Select at least one extension to share");
    let state = load(state_dir)?.context("No team remote is connected")?;
    let clone = clone_dir(state_dir);
    let mut seen = Vec::new();
    for item in items {
        let id = item.id();
        ensure!(
            !seen.contains(&id),
            "{id} is selected twice; pick one scope for it"
        );
        seen.push(id);
    }
    let mut changes = Vec::new();
    for item in items {
        for (path, local, remote) in file_pairs(item, &clone)? {
            let kind = match (read_bytes(&local)?, read_bytes(&remote)?) {
                (Bytes::Missing, Bytes::Missing) => continue,
                (Bytes::Missing, _) => ChangeKind::Remove,
                (_, Bytes::Missing) => ChangeKind::Add,
                (Bytes::Data(a), Bytes::Data(b)) if a == b => continue,
                _ => ChangeKind::Modify,
            };
            changes.push(FileChange { path, kind });
        }
    }
    ensure!(
        !changes.is_empty(),
        "Nothing to push: the remote already has these extensions as they are"
    );
    let branch = gitcmd::run_ok(&clone, &["symbolic-ref", "--short", "HEAD"])?
        .trim()
        .to_owned();
    Ok(SharePlan {
        url: redact(&state.url),
        branch,
        changes,
    })
}

/// Copy `items` into the clone, commit and push the branch, but only if the change is exactly
/// `reviewed`, the plan the human confirmed. Never forces; a rejected push is an error and the
/// clone is discarded (it is rebuilt on the next pull).
pub fn push(state_dir: &Path, items: &[Shared], reviewed: &SharePlan) -> Result<String> {
    pull(state_dir)?;
    let plan = plan_push(state_dir, items)?;
    ensure!(
        &plan == reviewed,
        "The remote or your files changed since you reviewed the list; review it again"
    );
    let state = load(state_dir)?.context("No team remote is connected")?;
    let (repo, dir) = open_clone(state_dir, &state)?;
    let sent = (|| -> Result<String> {
        for item in items {
            let target = dir.join(item.repo_path()?);
            match fs::symlink_metadata(&target) {
                Ok(meta) if meta.is_dir() => fs::remove_dir_all(&target)?,
                Ok(_) => fs::remove_file(&target)?,
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            files::install_copy(&item.unit, &target)?;
        }
        repo.stage_all()?;
        let body: String = items
            .iter()
            .map(|item| format!("- {}\n", item.id()))
            .collect();
        repo.commit(
            &format!("Share {} extension(s) from Yoke", items.len()),
            &body,
        )?;
        let reviewed = repo.push_plan()?;
        repo.push(&reviewed)
    })();
    match sent {
        Ok(_) => Ok(format!(
            "Pushed {} file(s) to {} ({})",
            plan.changes.len(),
            plan.url,
            plan.branch
        )),
        Err(error) => {
            let _ = discard_clone(state_dir);
            Err(error.context("Nothing was pushed; pull again and retry"))
        }
    }
}

#[cfg(test)]
mod tests;
