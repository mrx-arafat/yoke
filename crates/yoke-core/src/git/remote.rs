//! Branches, remotes and stashes. Nothing here forces: no force push, no forced switch, no
//! discarded changes. A stash is addressed by its commit id, so a stale page cannot act on the
//! wrong entry after the list shifted.
use anyhow::{bail, ensure, Context, Result};

use super::{ref_name, Repo, READ};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// `main` for a local branch, `origin/main` for a remote-tracking one.
    pub name: String,
    pub remote: bool,
    pub current: bool,
    pub upstream: Option<String>,
    /// For example `ahead 1, behind 2` or `gone`; empty when level.
    pub track: String,
    pub short: String,
    pub ts: i64,
    pub subject: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullStrategy {
    FastForwardOnly,
    Merge,
    Rebase,
}

impl PullStrategy {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "ff-only" => Some(Self::FastForwardOnly),
            "merge" => Some(Self::Merge),
            "rebase" => Some(Self::Rebase),
            _ => None,
        }
    }
}

/// Exactly what a push would do; compared again at confirmation so it cannot change in between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushPlan {
    pub branch: String,
    pub remote: String,
    /// The remote's URL with any embedded credentials removed.
    pub url: String,
    pub head: String,
    pub set_upstream: bool,
    pub ahead: u32,
    /// Up to 20 `<hash> <subject>` lines of what will be sent.
    pub commits: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stash {
    pub index: usize,
    /// The stash commit; the stable handle for apply, pop and drop.
    pub id: String,
    pub ts: i64,
    pub message: String,
}

fn strip_credentials(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
            let host = authority.rsplit('@').next().unwrap_or(authority);
            format!(
                "{scheme}://{host}{}{path}",
                if path.is_empty() { "" } else { "/" }
            )
        }
        None => url.to_owned(),
    }
}

impl Repo {
    fn config(&self, key: &str) -> Option<String> {
        let output = self
            .execute(true, false, &["config", "--get", key], &[], READ, 4096)
            .ok()?;
        let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (output.success && !value.is_empty()).then_some(value)
    }

    fn ensure_idle(&self, what: &str) -> Result<super::Status> {
        let status = self.status()?;
        if let Some(operation) = status.operation {
            bail!("a {operation} is unfinished in this repository; finish or abort it in a terminal (git {operation} --abort) before {what}");
        }
        Ok(status)
    }

    /// Local branches first (current, then most recent), then remote-tracking ones.
    pub fn branches(&self) -> Result<Vec<Branch>> {
        let format = "%(refname)%00%(refname:short)%00%(HEAD)%00%(upstream:short)%00%(upstream:track,nobracket)%00%(objectname:short)%00%(committerdate:unix)%00%(contents:subject)";
        let text = self.read(&[
            "for-each-ref",
            &format!("--format={format}"),
            "refs/heads",
            "refs/remotes",
        ])?;
        let mut branches: Vec<Branch> = text
            .lines()
            .filter_map(|line| {
                let f: Vec<&str> = line.splitn(8, '\0').collect();
                (f.len() == 8 && !f[0].ends_with("/HEAD")).then(|| Branch {
                    name: f[1].to_owned(),
                    remote: f[0].starts_with("refs/remotes/"),
                    current: f[2] == "*",
                    upstream: (!f[3].is_empty()).then(|| f[3].to_owned()),
                    track: f[4].to_owned(),
                    short: f[5].to_owned(),
                    ts: f[6].parse().unwrap_or(0),
                    subject: f[7].to_owned(),
                })
            })
            .collect();
        branches.sort_by(|a, b| {
            (a.remote, !a.current, -a.ts, &a.name).cmp(&(b.remote, !b.current, -b.ts, &b.name))
        });
        Ok(branches)
    }

    /// Switch to a local branch, or create a tracking branch from a remote one. Git refuses when
    /// local changes would be overwritten; nothing is discarded.
    pub fn switch(&self, name: &str) -> Result<()> {
        let name = ref_name(name)?;
        self.ensure_idle("switching branches")?;
        let branches = self.branches()?;
        let found = branches
            .iter()
            .find(|branch| branch.name == name)
            .with_context(|| format!("no branch named {name}; refresh the Changes page"))?;
        if found.remote {
            self.write(&["switch", "--track", name])?;
        } else {
            self.write(&["switch", "--no-guess", name])?;
        }
        Ok(())
    }

    /// Create `name` at `start` (default HEAD) and switch to it.
    pub fn create_branch(&self, name: &str, start: Option<&str>) -> Result<()> {
        let name = ref_name(name)?;
        self.ensure_idle("creating a branch")?;
        self.read(&["check-ref-format", "--branch", name])
            .map_err(|_| anyhow::anyhow!("{name:?} is not a valid branch name; avoid spaces, '..', '~', '^', ':' and a trailing '.lock'"))?;
        ensure!(
            self.config(&format!("branch.{name}.remote")).is_none()
                && !self.branches()?.iter().any(|b| !b.remote && b.name == name),
            "a branch named {name} already exists"
        );
        match start {
            Some(start) => {
                let start = ref_name(start)?;
                self.read(&[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{start}^{{commit}}"),
                ])
                .map_err(|_| {
                    anyhow::anyhow!("{start:?} is not a branch or commit in this repository")
                })?;
                self.write(&["switch", "-c", name, start])?;
            }
            None => {
                self.write(&["switch", "-c", name])?;
            }
        }
        Ok(())
    }

    fn remotes(&self) -> Result<Vec<String>> {
        Ok(self.read(&["remote"])?.lines().map(str::to_owned).collect())
    }

    /// Fetch every remote. Nothing local changes except remote-tracking branches.
    pub fn fetch(&self) -> Result<String> {
        ensure!(
            !self.remotes()?.is_empty(),
            "this repository has no remote to fetch from"
        );
        self.online(&["fetch", "--all"])
    }

    pub fn pull(&self, strategy: PullStrategy) -> Result<String> {
        let status = self.ensure_idle("pulling")?;
        ensure!(
            status.branch.is_some(),
            "HEAD is detached; switch to a branch first"
        );
        ensure!(
            status.upstream.is_some(),
            "this branch has no upstream; push it with upstream first"
        );
        if strategy != PullStrategy::FastForwardOnly {
            ensure!(
                status.tracked_clean(),
                "commit, stash or discard your tracked changes first; a merge or rebase pull needs a clean working tree"
            );
        }
        let args: &[&str] = match strategy {
            PullStrategy::FastForwardOnly => &["pull", "--ff-only"],
            PullStrategy::Merge => &["pull", "--no-rebase", "--no-edit"],
            PullStrategy::Rebase => &["pull", "--rebase"],
        };
        self.online(args).map_err(|error| match self.status().ok().and_then(|s| s.operation) {
            Some(operation) => anyhow::anyhow!("{error:#}; the {operation} is unfinished: resolve it in your editor, or return to where you were with `git {operation} --abort` in a terminal"),
            None => error,
        })
    }

    /// What pushing the current branch would send, or why there is nothing to push.
    pub fn push_plan(&self) -> Result<PushPlan> {
        let status = self.status()?;
        let branch = status
            .branch
            .clone()
            .context("HEAD is detached; switch to a branch first")?;
        let head = status
            .head
            .clone()
            .context("there are no commits to push yet")?;
        let (remote, set_upstream, range): (String, bool, Vec<String>) = if status
            .upstream
            .is_some()
        {
            let remote = self
                .config(&format!("branch.{branch}.remote"))
                .context("this branch's upstream has no remote")?;
            ensure!(
                remote != ".",
                "this branch tracks a local branch; there is nothing to push"
            );
            ensure!(
                status.ahead > 0,
                "nothing to push: {branch} has no commits its upstream lacks"
            );
            (remote, false, vec!["@{upstream}..HEAD".to_owned()])
        } else {
            let remotes = self.remotes()?;
            let remote = match remotes.as_slice() {
                [] => bail!("this repository has no remote; add one in a terminal (git remote add origin <url>)"),
                [only] => only.clone(),
                many if many.iter().any(|name| name == "origin") => "origin".to_owned(),
                many => bail!("several remotes ({}) and none is origin; push once from a terminal to choose", many.join(", ")),
            };
            let range = vec![
                "HEAD".to_owned(),
                "--not".to_owned(),
                format!("--remotes={remote}"),
            ];
            (remote, true, range)
        };
        let remote = ref_name(&remote)?.to_owned();
        let mut count = vec!["rev-list", "--count"];
        count.extend(range.iter().map(String::as_str));
        let ahead = self.read(&count)?.trim().parse().unwrap_or(0);
        let mut log = vec!["log", "--oneline", "-n", "20"];
        log.extend(range.iter().map(String::as_str));
        let commits = self.read(&log)?.lines().map(str::to_owned).collect();
        let url = self.read(&["remote", "get-url", &remote])?;
        Ok(PushPlan {
            branch,
            url: crate::redact::redact(&strip_credentials(url.trim())),
            remote,
            head,
            set_upstream,
            ahead,
            commits,
        })
    }

    /// Push exactly the reviewed plan. Never forces; a rejected push is reported, not retried.
    pub fn push(&self, reviewed: &PushPlan) -> Result<String> {
        ensure!(
            &self.push_plan()? == reviewed,
            "the branch or its remote changed since you reviewed this push; review it again"
        );
        if reviewed.set_upstream {
            self.online(&["push", "--set-upstream", &reviewed.remote, &reviewed.branch])
        } else {
            self.online(&["push"])
        }
    }

    pub fn stashes(&self) -> Result<Vec<Stash>> {
        let text = self.read(&["stash", "list", "--format=%H%x00%ct%x00%gs"])?;
        Ok(text
            .lines()
            .enumerate()
            .filter_map(|(index, line)| {
                let mut f = line.splitn(3, '\0');
                Some(Stash {
                    index,
                    id: f.next()?.to_owned(),
                    ts: f.next()?.parse().ok()?,
                    message: f.next()?.to_owned(),
                })
            })
            .collect())
    }

    fn stash_ref(&self, id: &str) -> Result<String> {
        ensure!(
            matches!(id.len(), 40 | 64) && id.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid stash id"
        );
        let index = self
            .stashes()?
            .iter()
            .position(|stash| stash.id == id)
            .context("that stash no longer exists; refresh the Changes page")?;
        Ok(format!("stash@{{{index}}}"))
    }

    /// Stash tracked changes (and untracked files when asked). Says so when there was nothing.
    pub fn stash_save(&self, message: &str, include_untracked: bool) -> Result<String> {
        let message: String = message.chars().filter(|c| !c.is_control()).collect();
        let message = message.trim();
        ensure!(
            message.chars().count() <= 200,
            "keep the stash message under 200 characters"
        );
        let mut args = vec!["stash", "push"];
        if include_untracked {
            args.push("--include-untracked");
        }
        if !message.is_empty() {
            args.extend(["-m", message]);
        }
        let output = self.write(&args)?;
        Ok(if output.contains("No local changes to save") {
            "There was nothing to stash.".to_owned()
        } else {
            "Changes stashed.".to_owned()
        })
    }

    pub fn stash_apply(&self, id: &str) -> Result<()> {
        self.write(&["stash", "apply", &self.stash_ref(id)?])
            .map(drop)
    }

    pub fn stash_pop(&self, id: &str) -> Result<()> {
        self.write(&["stash", "pop", &self.stash_ref(id)?])
            .map(drop)
    }

    pub fn stash_drop(&self, id: &str) -> Result<()> {
        self.write(&["stash", "drop", &self.stash_ref(id)?])
            .map(drop)
    }
}
