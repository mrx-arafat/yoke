//! Commit times for the console's Usage page, which matches them to recorded sessions.
use anyhow::{Context, Result};

use super::Repo;

/// One commit on the current branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitTime {
    pub hash: String,
    /// Author time in Unix seconds.
    pub authored_at: i64,
}

impl Repo {
    /// The newest `limit` commits reachable from `HEAD` (at most 5000), newest first. Fails on a
    /// repository without commits, like `git log` itself.
    pub fn commit_times(&self, limit: usize) -> Result<Vec<CommitTime>> {
        let count = format!("--max-count={}", limit.clamp(1, super::MAX_ENTRIES));
        let text = self.read(&["log", "HEAD", &count, "--format=%H %at", "--"])?;
        text.lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let (hash, at) = line.split_once(' ').context("unexpected git log line")?;
                Ok(CommitTime {
                    hash: hash.to_owned(),
                    authored_at: at.parse().context("unexpected git log author time")?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::base_command;
    use super::*;

    #[test]
    fn commit_times_reads_author_times_newest_first() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let git = |args: &[&str], date: Option<&str>| {
            let mut command = base_command(&root, false).unwrap();
            if let Some(date) = date {
                command.env("GIT_AUTHOR_DATE", date);
            }
            let output = command.args(args).output().unwrap();
            assert!(output.status.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"], None);
        for (key, value) in [
            ("user.name", "Fixture"),
            ("user.email", "fixture@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            git(&["config", key, value], None);
        }
        let repo = Repo::open(&root).unwrap();
        assert!(repo.commit_times(10).is_err(), "no commits yet");
        for at in ["1700000000", "1700003600"] {
            git(
                &["commit", "-q", "--allow-empty", "-m", at],
                Some(&format!("@{at} +0000")),
            );
        }
        let times = repo.commit_times(10).unwrap();
        assert_eq!(
            times.iter().map(|c| c.authored_at).collect::<Vec<_>>(),
            [1_700_003_600, 1_700_000_000]
        );
        assert_eq!(times[0].hash.len(), 40);
        assert_eq!(repo.commit_times(1).unwrap().len(), 1);
    }
}
