//! The project a session belongs to is fixed where the session starts.
//!
//! A hook payload reports the directory of the tool call, and the agent chooses that directory.
//! If it chose the project, `cd` into another tree (or a symlink out of the project, or a
//! subdirectory holding a `yoke.toml` nobody trusted) would swap the trusted policy for none.
//! So the first event of a session fixes an anchor and the project root found from it, the
//! store remembers both, and later events use the payload directory only to pick a trusted
//! nested policy below the anchor.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{params, OptionalExtension};

use super::{canonical_directory, load_policy, policy_root};
use crate::config::LoadedConfig;
use crate::store::Store;

/// A longer id is not remembered: it can only come from a forged payload.
const MAX_SESSION_ID: usize = 256;
/// Anchors older than this are dropped when a new one is written; the session rows they
/// belong to are long gone by then.
const RETENTION_SECS: i64 = 180 * 86_400;

/// Where a session started and the project it was given there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAnchor {
    /// Canonical directory the session started in.
    pub dir: PathBuf,
    /// The project identity: memory, plans and the Stop guard are keyed by this path.
    pub root: PathBuf,
    /// Why the anchor could not be remembered (state read-only or at an older schema). The
    /// event still runs anchored to `start`; later events of the session may not.
    pub not_remembered: Option<String>,
}

/// The anchor of `session`: the one stored by its first event, else one made from `start`
/// (the directory the agent reports as its project, or the first event's directory) and
/// stored now. The first writer wins, so concurrent hooks of one session agree.
/// An empty or oversized session id cannot be remembered and anchors at `start` each time.
pub fn session_anchor(
    store: &Store,
    agent: &str,
    session: &str,
    start: &Path,
) -> Result<SessionAnchor> {
    let remembers = !session.is_empty() && session.len() <= MAX_SESSION_ID;
    let mut problem = None;
    if remembers {
        match stored(store, agent, session) {
            Ok(Some(found)) => return Ok(found),
            Ok(None) => {}
            Err(error) => problem = Some(format!("{error:#}")),
        }
    }
    let dir = canonical_directory(start)?;
    let root = policy_root(store, &dir)?;
    let mut anchor = SessionAnchor {
        dir,
        root,
        not_remembered: problem,
    };
    if remembers && anchor.not_remembered.is_none() {
        match remember(store, agent, session, &anchor).and_then(|()| stored(store, agent, session))
        {
            Ok(Some(winner)) => anchor = winner,
            Ok(None) => {}
            Err(error) => anchor.not_remembered = Some(format!("{error:#}")),
        }
    }
    Ok(anchor)
}

fn stored(store: &Store, agent: &str, session: &str) -> Result<Option<SessionAnchor>> {
    Ok(store
        .connection()
        .query_row(
            "SELECT anchor,root FROM session_anchors WHERE agent=?1 AND session_id=?2",
            params![agent, session],
            |row| {
                Ok(SessionAnchor {
                    dir: PathBuf::from(row.get::<_, String>(0)?),
                    root: PathBuf::from(row.get::<_, String>(1)?),
                    not_remembered: None,
                })
            },
        )
        .optional()?)
}

fn remember(store: &Store, agent: &str, session: &str, anchor: &SessionAnchor) -> Result<()> {
    let (Some(dir), Some(root)) = (anchor.dir.to_str(), anchor.root.to_str()) else {
        anyhow::bail!("session paths must be UTF-8");
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0));
    store.transaction(|tx| {
        tx.execute(
            "DELETE FROM session_anchors WHERE created_at < ?1",
            [now - RETENTION_SECS],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO session_anchors(agent,session_id,anchor,root,created_at) VALUES (?1,?2,?3,?4,?5)",
            params![agent, session, dir, root, now],
        )?;
        Ok(())
    })
}

/// The policy that applies to one event.
#[derive(Debug)]
pub struct EventPolicy {
    pub loaded: LoadedConfig,
    /// The directory the trusted `yoke.toml` was read from; gate commands run here.
    pub policy_root: PathBuf,
    /// Files named `yoke.toml` nearer to the event than the policy, which no human trusted.
    /// They apply nothing; they are also listed among the snapshots so the user is told.
    pub ignored: Vec<PathBuf>,
}

/// The trusted policy for one event of a session. `event_cwd` only selects a trusted nested
/// policy at or below the anchor; the project identity is the anchor's, whatever the event
/// reports.
pub fn load_for_event(
    store: &Store,
    anchor: &SessionAnchor,
    event_cwd: &Path,
    home: &Path,
) -> Result<EventPolicy> {
    load_policy(store, &anchor.dir, &anchor.root, event_cwd, home)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::config::Mode;
    use crate::trust::prepare_project;

    struct Fixture {
        _temp: tempfile::TempDir,
        home: PathBuf,
        project: PathBuf,
        outside: PathBuf,
        store: Store,
    }

    impl Fixture {
        /// A trusted enforce policy with one deny gate at the project root.
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let base = temp.path().canonicalize().unwrap();
            let home = base.join("home");
            let project = base.join("project");
            let outside = base.join("outside");
            for dir in [home.join(".yoke"), project.join("sub"), outside.clone()] {
                fs::create_dir_all(dir).unwrap();
            }
            let store = Store::open(home.join(".yoke/state.db")).unwrap();
            let f = Self {
                _temp: temp,
                home,
                project,
                outside,
                store,
            };
            f.trust(
                &f.project,
                "mode='enforce'\n[[gate]]\nname='no-rm'\non='pre_tool_use'\naction='deny'\n",
            );
            f
        }

        fn trust(&self, dir: &Path, policy: &str) {
            fs::write(dir.join("yoke.toml"), policy).unwrap();
            prepare_project(&self.store, dir)
                .unwrap()
                .commit(&self.store)
                .unwrap();
        }

        fn event(&self, anchor: &SessionAnchor, cwd: &Path) -> (LoadedConfig, PathBuf) {
            let policy = load_for_event(&self.store, anchor, cwd, &self.home).unwrap();
            (policy.loaded, policy.policy_root)
        }

        fn start(&self, session: &str, dir: &Path) -> SessionAnchor {
            session_anchor(&self.store, "claude", session, dir).unwrap()
        }
    }

    #[test]
    fn stale_nested_policy_and_knowledge_raise_notifications_for_the_session_project() {
        let f = Fixture::new();
        let anchor = f.start("nested-notifications", &f.project);
        let nested = f.project.join("sub");
        fs::create_dir_all(nested.join(".yoke")).unwrap();
        fs::write(nested.join(".yoke/knowledge.md"), "Approved knowledge\n").unwrap();
        f.trust(&nested, "mode='enforce'");
        fs::write(nested.join("yoke.toml"), "mode='observe'").unwrap();
        fs::write(nested.join(".yoke/knowledge.md"), "Changed knowledge\n").unwrap();
        let (loaded, _) = f.event(&anchor, &nested);
        crate::notifications::sync(&f.store, &loaded, 1).unwrap();
        let rows = crate::notifications::list(&f.store, &f.project, false, 100).unwrap();
        assert_eq!(
            rows.iter().filter(|row| row.kind == "config_stale").count(),
            2
        );
        assert_eq!(loaded.config.mode, Mode::Enforce);
    }

    #[test]
    fn no_cwd_an_event_reports_moves_the_trusted_policy_or_the_project() {
        let f = Fixture::new();
        let anchor = f.start("s", &f.project);
        assert_eq!(anchor.root, f.project);
        // A yoke.toml nobody trusted, a directory outside the project, and a symlink that
        // leads out of it: none of them may turn the project's policy off.
        fs::write(f.project.join("sub/yoke.toml"), "").unwrap();
        std::os::unix::fs::symlink(&f.outside, f.project.join("link")).unwrap();
        for cwd in [
            f.project.join("sub"),
            f.outside.clone(),
            f.project.join("link"),
        ] {
            let (loaded, policy) = f.event(&anchor, &cwd);
            assert_eq!(loaded.config.mode, Mode::Enforce, "{}", cwd.display());
            assert_eq!(loaded.config.gates.len(), 1, "{}", cwd.display());
            assert_eq!(loaded.project_root, f.project, "{}", cwd.display());
            assert_eq!(policy, f.project, "{}", cwd.display());
        }
        // The planted file is reported, so the user learns why its settings do not apply.
        let (loaded, _) = f.event(&anchor, &f.project.join("sub"));
        let planted = f.project.join("sub/yoke.toml");
        assert!(loaded
            .snapshots
            .iter()
            .any(|s| s.file == planted && s.exists && !s.trusted));
    }

    #[test]
    fn a_session_keeps_the_directory_its_first_event_started_in() {
        let f = Fixture::new();
        let first = f.start("s", &f.project);
        // A later event that names another start (the agent's cwd changed, or a forged event)
        // gets the stored anchor; another session gets its own.
        let later = f.start("s", &f.outside);
        assert_eq!((later.dir, later.root), (first.dir.clone(), first.root));
        let other = f.start("t", &f.outside);
        assert_eq!(other.dir, f.outside);
        // A different agent with the same session id is a different session.
        let codex = session_anchor(&f.store, "codex", "s", &f.outside).unwrap();
        assert_eq!(codex.dir, f.outside);
    }

    #[test]
    fn a_trusted_nested_policy_below_the_anchor_applies_but_the_project_stays_one() {
        let f = Fixture::new();
        let nested = f.project.join("sub");
        f.trust(&nested, "mode='observe'");
        let anchor = f.start("s", &f.project);
        let (loaded, policy) = f.event(&anchor, &nested);
        assert_eq!(loaded.config.mode, Mode::Observe);
        assert_eq!(policy, nested);
        // Plans, memory and the Stop guard still see one project.
        assert_eq!(loaded.project_root, f.project);

        // A trusted policy the session did not start in or below is not picked by a cd.
        let elsewhere = f.outside.join("other");
        fs::create_dir(&elsewhere).unwrap();
        f.trust(&elsewhere, "mode='observe'");
        let (loaded, policy) = f.event(&anchor, &elsewhere);
        assert_eq!(loaded.config.mode, Mode::Enforce);
        assert_eq!(policy, f.project);

        // A session that started inside the nested directory is bound to it.
        let inside = f.start("n", &nested);
        assert_eq!(inside.root, nested);
        assert_eq!(f.event(&inside, &nested).0.config.mode, Mode::Observe);
    }

    #[test]
    fn a_session_without_a_usable_id_is_anchored_at_its_start_each_time() {
        let f = Fixture::new();
        let anonymous = f.start("", &f.project);
        assert_eq!(anonymous.dir, f.project);
        assert_eq!(f.start("", &f.outside).dir, f.outside);
        let oversized = "x".repeat(MAX_SESSION_ID + 1);
        assert_eq!(f.start(&oversized, &f.project).dir, f.project);
        assert_eq!(f.start(&oversized, &f.outside).dir, f.outside);
    }
}
