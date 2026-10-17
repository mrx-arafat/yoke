use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::config::{Config, LoadedConfig};
use crate::store::{Store, TrustRow};

mod anchor;
pub use anchor::{load_for_event, session_anchor, EventPolicy, SessionAnchor};

#[derive(Debug, Serialize)]
pub struct SnapshotStatus {
    pub scope: String,
    pub file: PathBuf,
    pub trusted: bool,
    pub stale: bool,
    /// Whether the file is on disk. An absent, untrusted file is nothing to review.
    pub exists: bool,
    pub disk_error: Option<String>,
}

#[derive(Debug)]
pub struct PendingSnapshot {
    pub file: PathBuf,
    pub sha256: String,
    pub content: String,
    pub previous: Option<String>,
}

#[derive(Debug)]
pub struct PreparedTrust {
    pub scope: String,
    pub root: PathBuf,
    pub snapshots: Vec<PendingSnapshot>,
}

impl PreparedTrust {
    /// Save captured, reviewed bytes atomically after the caller confirms a human approval.
    /// This primitive does not authenticate humans; the CLI owns that boundary.
    pub fn commit(&self, store: &Store) -> Result<()> {
        ensure!(
            matches!(self.scope.as_str(), "global" | "project"),
            "invalid trust scope"
        );
        ensure!(self.root.is_absolute(), "trust root must be absolute");
        let config_file = match self.scope.as_str() {
            "global" => self.root.join(".yoke/config.toml"),
            _ => self.root.join("yoke.toml"),
        };
        ensure!(
            self.snapshots
                .iter()
                .filter(|snapshot| snapshot.file == config_file)
                .count()
                == 1,
            "trust batch requires exactly one config snapshot"
        );
        let timestamp = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
        let mut seen = std::collections::BTreeSet::new();
        let mut rows = Vec::new();
        for snapshot in &self.snapshots {
            ensure!(seen.insert(&snapshot.file), "duplicate snapshot path");
            ensure!(
                snapshot.file == config_file
                    || (self.scope == "project"
                        && snapshot.file == self.root.join(".yoke/knowledge.md")),
                "snapshot is outside its trust scope"
            );
            ensure!(
                sha256(snapshot.content.as_bytes()) == snapshot.sha256,
                "snapshot hash mismatch for {}",
                snapshot.file.display()
            );
            if snapshot.file == config_file {
                Config::parse(&snapshot.content)
                    .context("validating config before trust commit")?;
            } else {
                validate_knowledge(&snapshot.content)?;
            }
            rows.push(TrustRow {
                scope: self.scope.clone(),
                root: path_key(&self.root)?.into(),
                file: path_key(&snapshot.file)?.into(),
                sha256: snapshot.sha256.clone(),
                content: snapshot.content.clone(),
                trusted_at: timestamp,
            });
        }
        store.put_trust_batch(&rows)
    }
}

/// Capture a global config without modifying trust state or executing commands.
pub fn prepare_global(store: &Store, home: &Path) -> Result<PreparedTrust> {
    let root = canonical_directory(home)?;
    let file = root.join(".yoke/config.toml");
    let snapshot = prepare_snapshot(store, "global", &root, &file, true)?;
    Ok(PreparedTrust {
        scope: "global".into(),
        root,
        snapshots: vec![snapshot],
    })
}

/// Capture config and optional knowledge together for the nearest project.
pub fn prepare_project(store: &Store, cwd: &Path) -> Result<PreparedTrust> {
    let root = project_root(store, cwd)?;
    let config_file = root.join("yoke.toml");
    let mut snapshots = vec![prepare_snapshot(
        store,
        "project",
        &root,
        &config_file,
        true,
    )?];
    let knowledge = root.join(".yoke/knowledge.md");
    match fs::symlink_metadata(&knowledge) {
        Ok(_) => snapshots.push(prepare_snapshot(
            store, "project", &root, &knowledge, false,
        )?),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", knowledge.display()))
        }
    }
    Ok(PreparedTrust {
        scope: "project".into(),
        root,
        snapshots,
    })
}

fn prepare_snapshot(
    store: &Store,
    scope: &str,
    root: &Path,
    file: &Path,
    config: bool,
) -> Result<PendingSnapshot> {
    let bytes = read_regular_file(file)?;
    let intact = |row: &TrustRow| sha256(row.content.as_bytes()) == row.sha256;
    let sha256 = sha256(&bytes);
    let content =
        String::from_utf8(bytes).with_context(|| format!("{} is not UTF-8", file.display()))?;
    if config {
        Config::parse(&content)
            .and_then(|config| config.check_hook_budgets())
            .with_context(|| format!("validating {}", file.display()))?;
    } else {
        validate_knowledge(&content)?;
    }
    // A damaged record is shown as having no previous text: it can still be replaced.
    let previous = store
        .get_trust(scope, path_key(root)?, path_key(file)?)?
        .filter(intact)
        .map(|row| row.content);
    Ok(PendingSnapshot {
        file: file.into(),
        sha256,
        content,
        previous,
    })
}

/// Load only persisted snapshots; on-disk config is inspected solely for staleness.
pub fn load_trusted(store: &Store, cwd: &Path, home: &Path) -> Result<LoadedConfig> {
    let cwd = canonical_directory(cwd)?;
    let identity = policy_root(store, &cwd)?;
    Ok(load_policy(store, &cwd, &identity, &cwd, home)?.loaded)
}

/// The trusted policy for one event, and the directory it was read from.
///
/// `identity` is the project the memory, plans and Stop guard are keyed by; it never follows
/// `event`. The policy is the nearest trusted `yoke.toml` on the way up from `event`, but only
/// while `event` is at or below `anchor`: an event that reports a directory outside the
/// anchor (another tree, a symlink out of the project) picks no policy of its own.
/// A nearer `yoke.toml` that no human trusted never replaces a trusted one above it; it is
/// listed among the snapshots so the user is told why its settings do not apply.
fn load_policy(
    store: &Store,
    anchor: &Path,
    identity: &Path,
    event: &Path,
    home: &Path,
) -> Result<EventPolicy> {
    let home = canonical_directory(home)?;
    let (policy, ignored) = select_policy(store, anchor, identity, event)?;
    let mut loaded = LoadedConfig {
        config: Config::default(),
        origins: Config::default_origins(),
        project_root: identity.to_path_buf(),
        snapshots: Vec::new(),
        knowledge: None,
    };
    for (scope, root, file) in [
        ("global", &home, home.join(".yoke/config.toml")),
        ("project", &policy, policy.join("yoke.toml")),
    ] {
        let row = trusted_row(store, scope, root, &file)?;
        loaded.snapshots.push(status(scope, &file, row.as_ref()));
        if let Some(row) = row {
            let config = Config::parse(&row.content)
                .with_context(|| format!("invalid trusted snapshot {}", file.display()))?;
            loaded.config.merge_from(config, &file, &mut loaded.origins);
        }
    }
    let knowledge_file = policy.join(".yoke/knowledge.md");
    let row = trusted_row(store, "project", &policy, &knowledge_file)?;
    loaded
        .snapshots
        .push(status("project", &knowledge_file, row.as_ref()));
    if let Some(row) = row {
        validate_knowledge(&row.content)?;
        loaded.knowledge = Some(row.content);
    }
    for file in &ignored {
        loaded.snapshots.push(status("project", file, None));
    }
    Ok(EventPolicy {
        loaded,
        policy_root: policy,
        ignored,
    })
}

/// The directory whose trusted `yoke.toml` governs `event`, and the untrusted `yoke.toml` files
/// that sit nearer to it. With no trusted file on the way up the project is the policy root, so
/// only the global policy applies.
fn select_policy(
    store: &Store,
    anchor: &Path,
    identity: &Path,
    event: &Path,
) -> Result<(PathBuf, Vec<PathBuf>)> {
    let event = event.canonicalize().ok().filter(|path| path.is_dir());
    let start = event
        .as_deref()
        .filter(|event| event.starts_with(anchor))
        .unwrap_or(anchor);
    let mut nearer = Vec::new();
    for dir in start.ancestors() {
        let file = dir.join("yoke.toml");
        if trusted_row(store, "project", dir, &file)?.is_some() {
            return Ok((dir.into(), nearer));
        }
        if fs::symlink_metadata(&file).is_ok() {
            nearer.push(file);
        }
    }
    // Below the project root only: files above it are not part of this project.
    nearer.retain(|file| file.starts_with(identity) && file != &identity.join("yoke.toml"));
    Ok((identity.into(), nearer))
}

/// Canonical project identity survives deletion of the trusted config on disk.
pub fn project_root(store: &Store, cwd: &Path) -> Result<PathBuf> {
    discover_root(cwd, home_directory().as_deref(), |root, file| {
        let (Some(root), Some(file)) = (root.to_str(), file.to_str()) else {
            return Ok(false);
        };
        Ok(store.get_trust("project", root, file)?.is_some())
    })
}

/// Where `yoke init` writes: the root of the project that `cwd` already belongs to, from the
/// files on disk alone, so it needs no state database.
pub fn config_home(cwd: &Path) -> Result<PathBuf> {
    discover_root(cwd, home_directory().as_deref(), |_, _| Ok(false))
}

fn home_directory() -> Option<PathBuf> {
    crate::config::home().ok()?.canonicalize().ok()
}

/// The nearest directory above `cwd` that holds a `yoke.toml` or has a trusted one on record,
/// else the repository root, else `cwd`. A repository in the home directory is a dotfiles
/// setup, not a project: counting it would make every directory below `home` that is not a
/// repository itself one project, with one shared memory and log.
fn discover_root(
    cwd: &Path,
    home: Option<&Path>,
    trusted: impl Fn(&Path, &Path) -> Result<bool>,
) -> Result<PathBuf> {
    let cwd = canonical_directory(cwd)?;
    let mut repository = None;
    for root in cwd.ancestors() {
        let file = root.join("yoke.toml");
        let on_disk = match fs::symlink_metadata(&file) {
            Ok(_) => true,
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Err(error) => {
                return Err(error).with_context(|| format!("discovering {}", file.display()))
            }
        };
        if on_disk || trusted(root, &file)? {
            return Ok(root.into());
        }
        if repository.is_none() && Some(root) != home && root.join(".git").exists() {
            repository = Some(root.to_path_buf());
        }
    }
    Ok(repository.unwrap_or(cwd))
}

/// The project whose trusted policy governs `cwd`. A `yoke.toml` nearer to `cwd` that no human
/// trusted never shadows the policy of a trusted project above it: a file an agent or a
/// checkout dropped into a subdirectory must not switch the gates off. With no trusted project
/// on the way up this is the nearest `yoke.toml`, as for `yoke trust`.
fn policy_root(store: &Store, cwd: &Path) -> Result<PathBuf> {
    let canonical = canonical_directory(cwd)?;
    for root in canonical.ancestors() {
        let file = root.join("yoke.toml");
        if trusted_row(store, "project", root, &file)?.is_some() {
            return Ok(root.into());
        }
    }
    project_root(store, &canonical)
}

fn snapshot(store: &Store, scope: &str, root: &Path, file: &Path) -> Result<Option<TrustRow>> {
    let row = store.get_trust(scope, path_key(root)?, path_key(file)?)?;
    if let Some(row) = &row {
        ensure!(
            sha256(row.content.as_bytes()) == row.sha256,
            "trusted snapshot hash mismatch for {}; its record in the state database was changed or damaged, so review the file and run yoke trust{} in your own terminal to replace it",
            file.display(),
            if scope == "global" { " --global" } else { "" }
        );
    }
    Ok(row)
}

/// The snapshot that applies to `file`: its own, else the one approved for the repository a
/// linked worktree belongs to when the worktree holds those exact bytes.
fn trusted_row(store: &Store, scope: &str, root: &Path, file: &Path) -> Result<Option<TrustRow>> {
    // Never alias an unrepresentable project to another project's lossy display name.
    // Ancestor discovery continues, so trusted parent and global policy still apply.
    if scope == "project" && (root.to_str().is_none() || file.to_str().is_none()) {
        return Ok(None);
    }
    match snapshot(store, scope, root, file)? {
        Some(row) => Ok(Some(row)),
        None if scope == "project" => inherited(store, root, file),
        None => Ok(None),
    }
}

/// `git worktree add` makes a directory with its own copy of a committed yoke.toml, which would
/// start untrusted and run without its gates. A human already approved that file for the
/// repository, so its approved bytes apply in every linked worktree until a human trusts the
/// worktree's own file. Editing or deleting the worktree's copy changes nothing, exactly as
/// for any trusted file, so the gates cannot be switched off from inside a worktree.
/// Nothing is written.
fn inherited(store: &Store, root: &Path, file: &Path) -> Result<Option<TrustRow>> {
    let Some(main) = main_worktree(root) else {
        return Ok(None);
    };
    let Ok(relative) = file.strip_prefix(root) else {
        return Ok(None);
    };
    snapshot(store, "project", &main, &main.join(relative))
}

/// The main worktree's root when `root` is a linked worktree, read from the files git leaves
/// behind (`.git` names a gitdir whose `commondir` leads back to the main repository).
fn main_worktree(root: &Path) -> Option<PathBuf> {
    let text = |file: PathBuf| {
        read_regular_file(&file)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
    };
    let link = text(root.join(".git"))?;
    let admin = root.join(link.strip_prefix("gitdir:")?.trim());
    let common = admin
        .join(text(admin.join("commondir"))?.trim())
        .canonicalize()
        .ok()?;
    (common.file_name()? == ".git")
        .then(|| common.parent().map(Path::to_path_buf))
        .flatten()
}

fn status(scope: &str, file: &Path, row: Option<&TrustRow>) -> SnapshotStatus {
    let (stale, exists, disk_error) = match read_regular_file(file) {
        Ok(bytes) => (
            row.is_some_and(|row| sha256(&bytes) != row.sha256),
            true,
            None,
        ),
        Err(error) => {
            let missing = error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == ErrorKind::NotFound);
            (
                row.is_some(),
                !missing,
                if missing && row.is_none() {
                    None
                } else {
                    Some(format!("{error:#}"))
                },
            )
        }
    };
    SnapshotStatus {
        scope: scope.into(),
        file: file.into(),
        trusted: row.is_some(),
        stale,
        exists,
        disk_error,
    }
}

fn read_regular_file(file: &Path) -> Result<Vec<u8>> {
    const LIMIT: u64 = 1024 * 1024;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    anyhow::bail!("safe trust file opening is unavailable on this platform");
    let opened = match options.open(file) {
        Ok(opened) => opened,
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => anyhow::bail!(
            "{} is a symbolic link; Yoke trusts only regular files, so replace the link with a copy of the file",
            file.display()
        ),
        Err(error) => {
            return Err(error).with_context(|| format!("opening {}", file.display()))
        }
    };
    let metadata = opened
        .metadata()
        .with_context(|| format!("reading {}", file.display()))?;
    ensure!(
        metadata.is_file() && metadata.len() <= LIMIT,
        "{} must be a regular file of at most 1 MiB",
        file.display()
    );
    let mut bytes = Vec::new();
    opened
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", file.display()))?;
    ensure!(
        bytes.len() as u64 <= LIMIT,
        "{} exceeds 1 MiB",
        file.display()
    );
    Ok(bytes)
}

fn canonical_directory(path: &Path) -> Result<PathBuf> {
    let path = path
        .canonicalize()
        .with_context(|| format!("resolving {}", path.display()))?;
    ensure!(path.is_dir(), "{} must be a directory", path.display());
    Ok(path)
}

fn path_key(path: &Path) -> Result<&str> {
    path.to_str().context("trust paths must be UTF-8")
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_knowledge(content: &str) -> Result<()> {
    ensure!(
        !content.contains('\0'),
        "knowledge must be UTF-8 text without NUL bytes"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Mode;

    struct Fixture {
        temp: tempfile::TempDir,
        home: PathBuf,
        project: PathBuf,
        store: Store,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let project = temp.path().join("project");
            fs::create_dir_all(home.join(".yoke")).unwrap();
            fs::create_dir_all(project.join(".yoke")).unwrap();
            let store = Store::open(home.join(".yoke/state.db")).unwrap();
            Self {
                temp,
                home,
                project,
                store,
            }
        }
        fn load(&self) -> LoadedConfig {
            load_trusted(&self.store, &self.project, &self.home).unwrap()
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_project_scope_keeps_global_enforcement_without_lossy_trust_keys() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let f = Fixture::new();
        fs::write(f.home.join(".yoke/config.toml"), "mode='enforce'").unwrap();
        prepare_global(&f.store, &f.home)
            .unwrap()
            .commit(&f.store)
            .unwrap();
        let project = f
            .temp
            .path()
            .join(OsString::from_vec(b"project-\xff".to_vec()));
        let lossy = PathBuf::from(project.to_string_lossy().into_owned());
        let content = "mode='observe'";
        f.store
            .put_trust_batch(&[TrustRow {
                scope: "project".into(),
                root: lossy.to_str().unwrap().into(),
                file: lossy.join("yoke.toml").to_str().unwrap().into(),
                sha256: sha256(content.as_bytes()),
                content: content.into(),
                trusted_at: 0,
            }])
            .unwrap();
        let loaded = load_policy(&f.store, &project, &project, &project, &f.home)
            .unwrap()
            .loaded;
        assert_eq!(loaded.config.mode, Mode::Enforce);
        assert!(!loaded
            .snapshots
            .iter()
            .any(|snapshot| snapshot.scope == "project" && snapshot.trusted));

        fs::write(f.project.join("yoke.toml"), "mode='enforce'").unwrap();
        prepare_project(&f.store, &f.project)
            .unwrap()
            .commit(&f.store)
            .unwrap();
        let nested = f
            .project
            .canonicalize()
            .unwrap()
            .join(OsString::from_vec(b"nested-\xff".to_vec()));
        let policy = load_policy(&f.store, &nested, &f.project, &nested, &f.home).unwrap();
        assert_eq!(policy.policy_root, f.project.canonicalize().unwrap());
        assert_eq!(policy.loaded.config.mode, Mode::Enforce);
    }

    #[test]
    fn untrusted_disk_never_contributes_settings_gates_or_knowledge() {
        let f = Fixture::new();
        fs::write(f.home.join(".yoke/config.toml"), "mode='enforce'").unwrap();
        fs::write(
            f.project.join("yoke.toml"),
            "invalid TOML [[! execute nothing",
        )
        .unwrap();
        fs::write(
            f.project.join(".yoke/knowledge.md"),
            "untrusted instructions",
        )
        .unwrap();
        let loaded = f.load();
        assert_eq!(loaded.config.mode, Mode::Observe);
        assert!(loaded.config.gates.is_empty());
        assert_eq!(loaded.knowledge, None);
        assert!(loaded.snapshots.iter().all(|status| !status.trusted));
        assert!(prepare_project(&f.store, &f.project).is_err());
    }

    #[test]
    fn snapshots_keep_exact_approved_bytes_and_origins_after_edit_and_delete() {
        let f = Fixture::new();
        let global = f.home.join(".yoke/config.toml");
        fs::write(
            &global,
            "mode='enforce'\n[log]\nretention_days=12\npayload_cap=512",
        )
        .unwrap();
        prepare_global(&f.store, &f.home)
            .unwrap()
            .commit(&f.store)
            .unwrap();
        let project = f.project.join("yoke.toml");
        let approved = "# exact CRLF\r\n[log]\r\npayload_cap=123\r\n[[gate]]\r\nname='safe'\r\non='stop'\r\nrun='true'\r\n";
        fs::write(&project, approved).unwrap();
        fs::write(
            f.project.join(".yoke/knowledge.md"),
            "trusted knowledge\r\n",
        )
        .unwrap();
        let pending = prepare_project(&f.store, &f.project).unwrap();
        assert_eq!(pending.snapshots[0].content.as_bytes(), approved.as_bytes());
        pending.commit(&f.store).unwrap();
        fs::write(&global, "mode='observe'").unwrap();
        fs::write(&project, "[[gate]]\nname='unsafe'\non='stop'\nrun='false'").unwrap();
        fs::remove_file(f.project.join(".yoke/knowledge.md")).unwrap();
        let loaded = f.load();
        assert_eq!(loaded.config.mode, Mode::Enforce);
        assert_eq!(loaded.config.log.retention_days, 12);
        assert_eq!(loaded.config.log.payload_cap, 123);
        assert_eq!(loaded.config.gates[0].run, "true");
        assert_eq!(loaded.knowledge.as_deref(), Some("trusted knowledge\r\n"));
        assert_eq!(
            loaded.origins["mode"],
            global.canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(
            loaded.origins["log.payload_cap"],
            project.canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(loaded.origins["stop.max_blocks"], "default");
        assert!(loaded
            .snapshots
            .iter()
            .all(|status| status.trusted && status.stale));
        fs::remove_file(&project).unwrap();
        fs::create_dir_all(f.project.join("nested")).unwrap();
        let nested = load_trusted(&f.store, &f.project.join("nested"), &f.home).unwrap();
        assert_eq!(nested.project_root, f.project.canonicalize().unwrap());
        assert_eq!(nested.config.gates[0].name, "safe");
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn preparation_and_failed_retrust_leave_old_snapshot_untouched() {
        let f = Fixture::new();
        let file = f.project.join("yoke.toml");
        fs::write(&file, "mode='enforce'").unwrap();
        let captured = prepare_project(&f.store, &f.project).unwrap();
        assert_eq!(f.load().config.mode, Mode::Observe);
        fs::write(&file, "mode='observe'").unwrap();
        captured.commit(&f.store).unwrap();
        assert_eq!(f.load().config.mode, Mode::Enforce);
        assert!(f.load().snapshots.iter().any(|status| status.stale));
        fs::write(
            &file,
            "[[gate]]\nname='invalid'\non='stop'\nrun='true'\nmatch_tool='['",
        )
        .unwrap();
        assert!(prepare_project(&f.store, &f.project).is_err());
        assert_eq!(f.load().config.mode, Mode::Enforce);
        fs::write(&file, "mode='observe'").unwrap();
        fs::write(f.project.join(".yoke/knowledge.md"), [0xff]).unwrap();
        assert!(prepare_project(&f.store, &f.project).is_err());
        assert_eq!(f.load().config.mode, Mode::Enforce);
        let mut forged = captured;
        forged.snapshots[0].content = "mode='observe'".into();
        assert!(forged.commit(&f.store).is_err());
        assert_eq!(f.load().config.mode, Mode::Enforce);
        fs::write(&file, vec![b' '; 1024 * 1024 + 1]).unwrap();
        assert!(read_regular_file(&file).is_err());
        let canonical_file = file.canonicalize().unwrap();
        let status = f.load();
        assert_eq!(status.config.mode, Mode::Enforce);
        assert!(status
            .snapshots
            .iter()
            .any(|s| s.file == canonical_file && s.disk_error.is_some()));
        assert!(prepare_project(&f.store, &f.project).is_err());
        #[cfg(unix)]
        {
            fs::remove_file(&file).unwrap();
            let target = f.project.join("external.toml");
            fs::write(&target, "mode='observe'").unwrap();
            std::os::unix::fs::symlink(&target, &file).unwrap();
            assert!(read_regular_file(&file).is_err());
            assert_eq!(f.load().config.mode, Mode::Enforce);
            fs::remove_file(&file).unwrap();
            use std::os::unix::ffi::OsStrExt;
            let path = std::ffi::CString::new(file.as_os_str().as_bytes()).unwrap();
            // The temporary fixture owns this path; no writer is opened.
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            assert!(read_regular_file(&file).is_err());
            assert_eq!(f.load().config.mode, Mode::Enforce);
        }
    }

    #[test]
    fn a_repository_in_the_home_directory_does_not_make_everything_below_it_one_project() {
        let f = Fixture::new();
        let home = f.home.canonicalize().unwrap();
        fs::create_dir(home.join(".git")).unwrap();
        let notes = home.join("notes");
        fs::create_dir(&notes).unwrap();
        let none = |_: &Path, _: &Path| Ok(false);
        assert_eq!(discover_root(&notes, Some(&home), none).unwrap(), notes);
        // A repository of its own below the home directory is still the project.
        let repo = home.join("code/proj");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir(repo.join("src")).unwrap();
        assert_eq!(
            discover_root(&repo.join("src"), Some(&home), none).unwrap(),
            repo
        );
    }

    #[test]
    fn a_linked_worktree_applies_the_policy_approved_for_its_repository_until_trusted_itself() {
        let f = Fixture::new();
        let approved = "mode='enforce'";
        fs::write(f.project.join("yoke.toml"), approved).unwrap();
        prepare_project(&f.store, &f.project)
            .unwrap()
            .commit(&f.store)
            .unwrap();
        // The shape `git worktree add` leaves: a `.git` file naming a gitdir whose commondir
        // leads back to the main repository.
        let main = f.project.canonicalize().unwrap();
        let admin = main.join(".git/worktrees/wt");
        fs::create_dir_all(&admin).unwrap();
        fs::write(admin.join("commondir"), "../..\n").unwrap();
        let worktree = f.temp.path().join("wt");
        fs::create_dir(&worktree).unwrap();
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", admin.display()),
        )
        .unwrap();

        fs::write(worktree.join("yoke.toml"), approved).unwrap();
        let loaded = load_trusted(&f.store, &worktree, &f.home).unwrap();
        assert_eq!(loaded.project_root, worktree.canonicalize().unwrap());
        assert_eq!(loaded.config.mode, Mode::Enforce);
        assert!(loaded.snapshots.iter().any(|s| s.trusted && !s.stale));

        // Editing or deleting the worktree's copy must not switch the gates off: the approved
        // bytes keep applying, flagged stale like any trusted file that changed on disk.
        fs::write(worktree.join("yoke.toml"), "mode='observe'").unwrap();
        let loaded = load_trusted(&f.store, &worktree, &f.home).unwrap();
        assert_eq!(loaded.config.mode, Mode::Enforce);
        assert!(loaded.snapshots.iter().any(|s| s.trusted && s.stale));
        fs::remove_file(worktree.join("yoke.toml")).unwrap();
        assert_eq!(
            load_trusted(&f.store, &worktree, &f.home)
                .unwrap()
                .config
                .mode,
            Mode::Enforce
        );

        // A human who trusts the worktree's own file replaces the inherited policy.
        fs::write(worktree.join("yoke.toml"), "mode='observe'").unwrap();
        prepare_project(&f.store, &worktree)
            .unwrap()
            .commit(&f.store)
            .unwrap();
        let loaded = load_trusted(&f.store, &worktree, &f.home).unwrap();
        assert_eq!(loaded.config.mode, Mode::Observe);
    }

    #[test]
    fn a_damaged_record_can_be_replaced_and_a_symlinked_config_is_explained() {
        let f = Fixture::new();
        let file = f.project.join("yoke.toml");
        let approved = "\u{feff}mode='enforce'\r\n";
        fs::write(&file, approved).unwrap();
        prepare_project(&f.store, &f.project)
            .unwrap()
            .commit(&f.store)
            .unwrap();
        assert_eq!(f.load().config.mode, Mode::Enforce);
        // The record's text no longer matches its hash: hooks cannot read the policy, and the
        // error names the command that repairs it.
        f.store
            .connection()
            .execute(
                "UPDATE trust SET content='mode=\"observe\"' WHERE scope='project'",
                [],
            )
            .unwrap();
        let error = load_trusted(&f.store, &f.project, &f.home).unwrap_err();
        assert!(format!("{error:#}").contains("run yoke trust"), "{error:#}");
        // Trusting again replaces it instead of failing on the damaged previous text.
        let repair = prepare_project(&f.store, &f.project).unwrap();
        assert_eq!(repair.snapshots[0].previous, None);
        assert_eq!(repair.snapshots[0].content, approved);
        repair.commit(&f.store).unwrap();
        assert_eq!(f.load().config.mode, Mode::Enforce);

        fs::remove_file(&file).unwrap();
        let target = f.project.join("real.toml");
        fs::write(&target, approved).unwrap();
        std::os::unix::fs::symlink(&target, &file).unwrap();
        let error = prepare_project(&f.store, &f.project).unwrap_err();
        assert!(format!("{error:#}").contains("symbolic link"), "{error:#}");
    }

    #[test]
    fn nearest_project_boundary_and_canonical_identity_are_respected() {
        let f = Fixture::new();
        fs::write(f.project.join("yoke.toml"), "mode='enforce'").unwrap();
        prepare_project(&f.store, &f.project)
            .unwrap()
            .commit(&f.store)
            .unwrap();
        let nested = f.project.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("yoke.toml"), "mode='observe'").unwrap();
        // A file nobody trusted must not shadow the policy a human approved for the project.
        let loaded = load_trusted(&f.store, &nested, &f.home).unwrap();
        assert_eq!(loaded.project_root, f.project.canonicalize().unwrap());
        assert_eq!(loaded.config.mode, Mode::Enforce);
        let ignored = nested.canonicalize().unwrap().join("yoke.toml");
        assert!(loaded
            .snapshots
            .iter()
            .any(|s| s.file == ignored && s.exists && !s.trusted));
        // Once a human trusts the nested file it is a project of its own.
        prepare_project(&f.store, &nested)
            .unwrap()
            .commit(&f.store)
            .unwrap();
        let loaded = load_trusted(&f.store, &nested, &f.home).unwrap();
        assert_eq!(loaded.project_root, nested.canonicalize().unwrap());
        assert_eq!(loaded.config.mode, Mode::Observe);
        #[cfg(unix)]
        {
            let alias = f.temp.path().join("alias");
            std::os::unix::fs::symlink(&f.project, &alias).unwrap();
            let loaded = load_trusted(&f.store, &alias, &f.home).unwrap();
            assert_eq!(loaded.project_root, f.project.canonicalize().unwrap());
            assert_eq!(loaded.config.mode, Mode::Enforce);
        }
    }
}
