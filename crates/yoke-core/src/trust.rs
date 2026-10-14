use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::config::{Config, LoadedConfig};
use crate::store::{Store, TrustRow};

#[derive(Debug, Serialize)]
pub struct SnapshotStatus {
    pub scope: String,
    pub file: PathBuf,
    pub trusted: bool,
    pub stale: bool,
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
    let previous = snapshot(store, scope, root, file)?.map(|row| row.content);
    Ok(PendingSnapshot {
        file: file.into(),
        sha256,
        content,
        previous,
    })
}

/// Load only persisted snapshots; on-disk config is inspected solely for staleness.
pub fn load_trusted(store: &Store, cwd: &Path, home: &Path) -> Result<LoadedConfig> {
    let home = canonical_directory(home)?;
    let root = project_root(store, cwd)?;
    let mut loaded = LoadedConfig {
        config: Config::default(),
        origins: Config::default_origins(),
        project_root: root.clone(),
        snapshots: Vec::new(),
        knowledge: None,
    };
    for (scope, root, file) in [
        ("global", &home, home.join(".yoke/config.toml")),
        ("project", &root, root.join("yoke.toml")),
    ] {
        let row = snapshot(store, scope, root, &file)?;
        loaded.snapshots.push(status(scope, &file, row.as_ref()));
        if let Some(row) = row {
            let config = Config::parse(&row.content)
                .with_context(|| format!("invalid trusted snapshot {}", file.display()))?;
            loaded.config.merge_from(config, &file, &mut loaded.origins);
        }
    }
    let knowledge_file = root.join(".yoke/knowledge.md");
    let row = snapshot(store, "project", &root, &knowledge_file)?;
    loaded
        .snapshots
        .push(status("project", &knowledge_file, row.as_ref()));
    if let Some(row) = row {
        validate_knowledge(&row.content)?;
        loaded.knowledge = Some(row.content);
    }
    Ok(loaded)
}

/// Canonical project identity survives deletion of the trusted config on disk.
pub fn project_root(store: &Store, cwd: &Path) -> Result<PathBuf> {
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
        if on_disk
            || store
                .get_trust("project", path_key(root)?, path_key(&file)?)?
                .is_some()
        {
            return Ok(root.into());
        }
        if repository.is_none() && root.join(".git").exists() {
            repository = Some(root.to_path_buf());
        }
    }
    Ok(repository.unwrap_or(cwd))
}

fn snapshot(store: &Store, scope: &str, root: &Path, file: &Path) -> Result<Option<TrustRow>> {
    let row = store.get_trust(scope, path_key(root)?, path_key(file)?)?;
    if let Some(row) = &row {
        ensure!(
            sha256(row.content.as_bytes()) == row.sha256,
            "trusted snapshot hash mismatch for {}",
            file.display()
        );
    }
    Ok(row)
}

fn status(scope: &str, file: &Path, row: Option<&TrustRow>) -> SnapshotStatus {
    let (stale, disk_error) = match read_regular_file(file) {
        Ok(bytes) => (row.is_some_and(|row| sha256(&bytes) != row.sha256), None),
        Err(error) => {
            let missing = error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == ErrorKind::NotFound);
            (
                row.is_some(),
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
    let opened = options
        .open(file)
        .with_context(|| format!("opening {}", file.display()))?;
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
    fn nearest_project_boundary_and_canonical_identity_are_respected() {
        let f = Fixture::new();
        fs::write(f.project.join("yoke.toml"), "mode='enforce'").unwrap();
        prepare_project(&f.store, &f.project)
            .unwrap()
            .commit(&f.store)
            .unwrap();
        let nested = f.project.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("yoke.toml"), "mode='enforce'").unwrap();
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
