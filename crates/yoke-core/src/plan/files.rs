use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};

use super::{EvidenceStatus, PlanHeader, PlanRecord};
use crate::engine::execute_bounded;

const MAX_ARTIFACT_BYTES: u64 = 10 * 1024 * 1024;

pub(super) fn project(path: &Path) -> Result<PathBuf> {
    let root = path.canonicalize().context("resolving project root")?;
    ensure!(root.is_dir(), "project root must be a directory; change into the intended project directory and retry the plan command");
    ensure!(root.to_str().is_some(), "project root must be UTF-8; rename its non-UTF-8 directory components before retrying the plan command");
    Ok(root)
}

pub(super) fn validate_slug(slug: &str) -> Result<()> {
    ensure!(
        !slug.is_empty()
            && slug.len() <= 80
            && slug
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            && !slug.starts_with('-')
            && !slug.ends_with('-'),
        "slug must be 1-80 lowercase letters, digits, or interior hyphens"
    );
    Ok(())
}

fn relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

pub(super) fn new_path(root: &Path, directory: &str, slug: &str) -> Result<PathBuf> {
    ensure!(
        relative(Path::new(directory)),
        "plans directory must be a relative path without traversal; correct plans.dir in yoke.toml and run yoke trust in your own terminal before retrying"
    );
    let directory = root.join(directory);
    let ancestor = directory
        .ancestors()
        .find(|path| path.exists())
        .context("plans directory has no existing ancestor")?;
    ensure!(
        ancestor.canonicalize()?.starts_with(root),
        "plans directory escapes project; restore a project-local plans.dir directory without escaping symlinks, then retry yoke plan new"
    );
    fs::create_dir_all(&directory)?;
    let directory = directory.canonicalize()?;
    ensure!(
        directory.starts_with(root),
        "plans directory escapes project; restore a project-local plans.dir directory without escaping symlinks, then retry yoke plan new"
    );
    Ok(directory.join(format!("{slug}.md")))
}

fn resolved(plan: &PlanRecord) -> Result<PathBuf> {
    let root = project(Path::new(&plan.project))?;
    let path = Path::new(&plan.file)
        .canonicalize()
        .context("resolving plan file")?;
    ensure!(path.starts_with(&root), "plan file escapes project; restore the recorded plan file inside its project, then retry the plan command");
    Ok(path)
}

/// Take exclusive advisory ownership independent of atomic saves to the plan file.
/// The OS releases it when the owning process exits, so a killed check cannot hold it;
/// `None` means a live process still owns the check.
pub(super) fn claim_check(plan: &PlanRecord) -> Result<Option<fs::File>> {
    resolved(plan)?;
    let output = git(
        Path::new(&plan.project),
        &["rev-parse", "--absolute-git-dir"],
    )?;
    let directory = PathBuf::from(OsString::from_vec(
        output.strip_suffix(b"\n").unwrap_or(&output).to_vec(),
    ));
    ensure!(
        directory.is_absolute(),
        "Git metadata path must be absolute"
    );
    // Git metadata stays outside fingerprints/review. Never unlink this file on release:
    // future owners must lock the same inode, including after the plan is atomically saved.
    let path = directory
        .canonicalize()?
        .join(format!("yoke-check-{}.lock", hash(plan.file.as_bytes())));
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .context("opening plan check ownership lock")?;
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no arguments or memory-safety preconditions.
    let uid = unsafe { libc::geteuid() };
    ensure!(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == uid
            && metadata.mode() & 0o077 == 0,
        "plan check lock must be a private, singly linked regular file owned by this user; stop other plan checks, back up the lock entry in Git metadata, restore a regular file owned by you with mode 0600 and one link, then retry yoke plan check"
    );
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(error)) => {
            Err(error).context("locking plan check ownership file")
        }
    }
}

pub(super) fn read(plan: &PlanRecord) -> Result<String> {
    let path = resolved(plan)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= 1024 * 1024,
        "plan must be a regular file of at most 1 MiB; back up the original entry and restore the plan's Markdown file within that limit, then have a human retry yoke plan approve <slug> in the project terminal"
    );
    let mut text = String::new();
    file.take(1024 * 1024 + 1)
        .read_to_string(&mut text)
        .context("reading plan")?;
    ensure!(
        text.len() <= 1024 * 1024,
        "plan grew beyond 1 MiB; shorten it before retrying"
    );
    Ok(text)
}

pub(super) fn parse(content: &str) -> Result<(String, PlanHeader)> {
    parse_header(content).map_err(|error| anyhow::anyhow!("{error:#}; edit the plan file's +++ TOML header, then have a human run yoke plan approve <slug> in that project's terminal before retrying checks"))
}

fn parse_header(content: &str) -> Result<(String, PlanHeader)> {
    let rest = content
        .strip_prefix("+++\r\n")
        .or_else(|| content.strip_prefix("+++\n"))
        .context("plan must begin with a +++ TOML header")?;
    let mut end = 0;
    let mut found = false;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "+++" {
            found = true;
            break;
        }
        end += line.len();
    }
    ensure!(found, "plan TOML header is missing its closing +++");
    let raw = rest[..end].to_owned();
    let header: PlanHeader = toml::from_str(&raw).context("parsing plan header")?;
    ensure!(
        !header.title.trim().is_empty(),
        "plan title must not be empty"
    );
    ensure!(
        header.checks.iter().any(|check| check.blocking) || header.review,
        "plan needs a blocking check or required review"
    );
    let mut names = BTreeSet::new();
    for check in &header.checks {
        ensure!(
            check.inputs.len() <= 64,
            "check {} has more than 64 inputs; split it into smaller checks",
            check.name
        );
        let mut inputs = BTreeSet::new();
        for input in &check.inputs {
            ensure!(
                relative(Path::new(input)) && inputs.insert(input),
                "check {} inputs must be unique project-relative file paths without traversal",
                check.name
            );
        }
        ensure!(
            !check.name.trim().is_empty() && check.name != "review" && names.insert(&check.name),
            "check names must be unique, nonempty, and not reserved review"
        );
        ensure!(
            !check.command.trim().is_empty(),
            "check {} requires a command",
            check.name
        );
        ensure!(
            check.timeout > 0,
            "check {} timeout must be positive",
            check.name
        );
        for artifact in &check.artifacts {
            let path = Path::new(artifact);
            ensure!(
                relative(path) && image_extension(path),
                "artifact must be a relative PNG, JPEG, or WebP path without traversal"
            );
        }
    }
    super::acceptance::validate(&header)?;
    Ok((raw, header))
}

pub(super) fn approved(plan: &PlanRecord) -> Result<PlanHeader> {
    let Some(frozen) = plan.approved_header.as_ref() else {
        return Err(super::wrong_state(plan, "it has no approved header yet"));
    };
    let (raw, header) = parse(&read(plan)?)?;
    let raw = approval_header(plan, raw, &header)?;
    ensure!(
        &raw == frozen,
        "plan header changed or check inputs changed; human re-approval required: run yoke plan approve in your own terminal to approve the current files"
    );
    Ok(header)
}

/// Append deterministic, human-visible input hashes without rewriting the editable plan.
pub(super) fn approval_header(
    plan: &PlanRecord,
    mut raw: String,
    header: &PlanHeader,
) -> Result<String> {
    let root = project(Path::new(&plan.project))?;
    let mut total = 0usize;
    let mut hashes = BTreeMap::new();
    for name in header.checks.iter().flat_map(|check| &check.inputs) {
        if hashes.contains_key(name) {
            continue;
        }
        ensure!(
            relative(Path::new(name)),
            "check input must stay inside the project; correct check.inputs in the plan header to project-relative files and have a human retry yoke plan approve <slug>"
        );
        let path = root.join(name);
        let parent = path
            .parent()
            .context("check input has no parent")?
            .canonicalize()?;
        ensure!(
            parent.starts_with(&root),
            "check input escapes project; use a regular project-local file"
        );
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .with_context(|| {
                format!(
                    "open check input {name}; restore the file or update inputs before approval"
                )
            })?;
        ensure!(
            file.metadata()?.is_file(),
            "check input {name} must be a regular file; restore a regular project-local file or correct check.inputs, then have a human retry yoke plan approve <slug>"
        );
        let mut bytes = Vec::new();
        file.take(10 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        total = total.saturating_add(bytes.len());
        ensure!(
            bytes.len() <= 10 * 1024 * 1024 && total <= 64 * 1024 * 1024,
            "check inputs exceed 10 MiB per file or 64 MiB total; split checks or narrow inputs"
        );
        hashes.insert(name, hash(&bytes));
    }
    for (name, hash) in hashes {
        raw.push_str(&format!(
            "\n# yoke-approved-input {} sha256={hash}\n",
            serde_json::to_string(name)?
        ));
    }
    Ok(raw)
}

pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = execute_bounded(
        crate::process::git()?.current_dir(root).args(args),
        &[],
        Duration::from_secs(30),
        16 * 1024 * 1024,
    )?;
    ensure!(
        output.success && !output.timed_out && !output.stdout_truncated,
        "git fingerprint command failed or exceeded its bound; inspect git status --short, restore readable Git metadata and narrow the working changes before retrying the plan command: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

/// Commit id of HEAD, pinned as the review base when a plan starts.
pub(super) fn head(project: &Path) -> Result<String> {
    let output = git(project, &["rev-parse", "--verify", "HEAD"])
        .context("plan start needs a git repository with at least one commit")?;
    let id = String::from_utf8(output)?.trim().to_owned();
    ensure!(
        !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "git returned an invalid HEAD commit id"
    );
    Ok(id)
}

fn toplevel(project_path: &Path) -> Result<PathBuf> {
    let cwd = project(project_path)?;
    let top = git(&cwd, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(OsString::from_vec(
        top.strip_suffix(b"\n").unwrap_or(&top).to_vec(),
    )))
}

/// Above this size a working-tree file is identified by its size and modification time, so
/// hashing stays bounded per entry.
const STREAMED_HASH_LIMIT: u64 = 64 * 1024 * 1024;

/// Fingerprint HEAD and every nonignored path whose content differs from it.
pub fn fingerprint(project_path: &Path) -> Result<String> {
    let root = toplevel(project_path)?;
    let mut hash = Sha256::new();
    field(&mut hash, &git(&root, &["rev-parse", "--verify", "HEAD"])?);
    hash_changes(&mut hash, &root, "HEAD")?;
    Ok(format!("{:x}", hash.finalize()))
}

/// The nonignored working-tree content relative to commit `base`, independent of HEAD and the
/// index, so committing exactly these bytes keeps it. `None` when `base` no longer exists.
/// It reads files and git metadata only and never writes git objects.
pub fn content_fingerprint(project_path: &Path, base: &str) -> Result<Option<String>> {
    ensure!(
        !base.is_empty() && base.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "content base must be a commit id"
    );
    let root = toplevel(project_path)?;
    let commit = format!("{base}^{{commit}}");
    let found = execute_bounded(
        crate::process::git()?.current_dir(&root).args([
            "rev-parse",
            "--verify",
            "--quiet",
            &commit,
        ]),
        &[],
        Duration::from_secs(30),
        4096,
    )?;
    if found.exit_code == Some(1) {
        return Ok(None);
    }
    ensure!(
        found.success && !found.timed_out,
        "git could not resolve commit {base}; inspect git status --short and restore access to the recorded commit before retrying the plan command; if Git still fails, report this diagnostic with git --version and yoke --version"
    );
    let mut hash = Sha256::new();
    field(&mut hash, base.as_bytes());
    hash_changes(&mut hash, &root, base)?;
    Ok(Some(format!("{:x}", hash.finalize())))
}

/// Hash every nonignored path that differs from `base`, in path order, by type, mode and the
/// bytes on disk. Git supplies only paths, modes and commit ids (`--raw`), never diff text, so
/// each entry costs a bounded amount, the result ignores HEAD and the index, and nothing is
/// written to the repository.
fn hash_changes(hash: &mut Sha256, root: &Path, base: &str) -> Result<()> {
    let raw = git(
        root,
        &[
            "diff",
            "--raw",
            "-z",
            "--no-renames",
            "--no-abbrev",
            base,
            "--",
        ],
    )?;
    let mut changes = BTreeMap::new();
    let mut fields = raw.split(|byte| *byte == 0);
    while let Some(meta) = fields.next().filter(|meta| !meta.is_empty()) {
        let path = fields.next().context("git raw diff entry lacks a path")?;
        let meta = std::str::from_utf8(meta).context("git raw diff entry is not text")?;
        let parts: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        ensure!(parts.len() == 5, "unexpected git raw diff entry");
        let tracked = (
            parts[1].to_owned(),
            parts[3].to_owned(),
            parts[4].to_owned(),
        );
        changes.insert(path.to_vec(), Some(tracked));
    }
    let untracked = git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        // Nested repositories are listed as directories with a trailing slash.
        changes.insert(path.strip_suffix(b"/").unwrap_or(path).to_vec(), None);
    }
    for (raw_path, tracked) in changes {
        let path = PathBuf::from(OsString::from_vec(raw_path.clone()));
        ensure!(relative(&path), "invalid path returned by git");
        field(hash, &raw_path);
        match tracked {
            Some((mode, _, status)) if mode == "000000" && status != "U" => {
                let full = root.join(&path);
                // A staged deletion can still exist on disk, even as an ignored file.
                match fs::symlink_metadata(&full) {
                    Ok(metadata) => hash_listed_entry(hash, &full, &metadata)?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        field(hash, b"deleted");
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Some((mode, id, _)) if mode == "160000" && id.bytes().any(|byte| byte != b'0') => {
                field(hash, b"160000");
                field(hash, id.as_bytes());
            }
            _ => hash_entry(hash, &root.join(&path))?,
        }
    }
    Ok(())
}

/// One working-tree entry by type: a file by mode and streamed content (size and modification
/// time above the limit), a link by target, and a nested repository by its checked-out commit,
/// as git records a submodule.
fn hash_entry(hash: &mut Sha256, full: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(full) {
        Ok(metadata) => metadata,
        // Removed after git listed it: changed, like any other concurrent edit.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            field(hash, b"vanished");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    hash_listed_entry(hash, full, &metadata)
}

/// The entry may change type between listing and opening, so validate the opened descriptor.
fn hash_listed_entry(hash: &mut Sha256, full: &Path, metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let kind = metadata.file_type();
    if kind.is_symlink() {
        field(hash, b"120000");
        field(hash, fs::read_link(full)?.as_os_str().as_bytes());
    } else if kind.is_file() {
        let executable = metadata.permissions().mode() & 0o111 != 0;
        field(hash, if executable { b"100755" } else { b"100644" });
        if metadata.len() > STREAMED_HASH_LIMIT {
            field(hash, b"large");
            field(hash, &metadata.len().to_le_bytes());
            field(hash, &metadata.mtime().to_le_bytes());
            field(hash, &metadata.mtime_nsec().to_le_bytes());
        } else {
            let file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(full)
                .context("working-tree entry changed or cannot be opened safely; restore a regular file and retry the command")?;
            let opened = file.metadata()?;
            ensure!(opened.is_file(), "working-tree entry changed to a special file while hashing; restore a regular file and retry the command");
            hash_content(hash, file, opened.len())?;
        }
    } else if kind.is_dir() {
        field(hash, b"160000");
        field(hash, &nested_head(full)?);
    } else {
        field(hash, b"special");
        field(hash, &(metadata.mode() & 0o170000).to_le_bytes());
    }
    Ok(())
}

/// Stream up to the limit of `content`, which was `length` bytes long when it was listed.
fn hash_content(hash: &mut Sha256, content: impl Read, length: u64) -> Result<()> {
    let mut digest = Sha256::new();
    let copied = std::io::copy(&mut content.take(STREAMED_HASH_LIMIT + 1), &mut digest)?;
    field(hash, &digest.finalize());
    if copied != length {
        // A file still being written (a log, build output) counts as changed, never as an error.
        field(hash, b"changed while hashed");
    }
    Ok(())
}

fn nested_head(directory: &Path) -> Result<Vec<u8>> {
    if !directory.join(".git").exists() {
        return Ok(b"directory".to_vec());
    }
    let output = execute_bounded(
        crate::process::git()?.current_dir(directory).args([
            "rev-parse",
            "--verify",
            "--quiet",
            "HEAD^{commit}",
        ]),
        &[],
        Duration::from_secs(30),
        4096,
    )?;
    if output.exit_code == Some(1) {
        return Ok(b"no commit".to_vec());
    }
    ensure!(
        output.success && !output.timed_out,
        "git could not read the nested repository {}; restore readable Git metadata in that directory and retry the plan command; if Git still fails, report this diagnostic with git --version and yoke --version",
        directory.display()
    );
    Ok(output.stdout.trim_ascii_end().to_vec())
}

fn image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp"
            )
        })
}

/// Validate declared artifacts and return each canonical path with its SHA-256 at capture time.
pub(super) fn artifacts(root: &Path, declared: &[String]) -> Result<Vec<(String, String)>> {
    declared
        .iter()
        .map(|artifact| {
            let path = root
                .join(artifact)
                .canonicalize()
                .with_context(|| format!("missing declared artifact {artifact}"))?;
            ensure!(
                path.starts_with(root) && image_extension(&path),
                "artifact escapes project or is not an image; regenerate the declared PNG, JPEG or WebP inside the project and rerun yoke plan check <slug> --agent <claude|codex>"
            );
            let metadata = fs::metadata(&path)?;
            ensure!(
                metadata.is_file() && metadata.len() <= MAX_ARTIFACT_BYTES,
                "artifact must be a regular image of at most 10 MiB; regenerate a smaller project-local PNG, JPEG or WebP and rerun yoke plan check <slug> --agent <claude|codex>"
            );
            let hash = file_hash(&path)?;
            let path = path
                .to_str()
                .context("artifact path must be UTF-8")?
                .to_owned();
            Ok((path, hash))
        })
        .collect()
}

/// Hash one regular, non-symlink file of at most 10 MiB through a single open handle.
fn file_hash(path: &Path) -> Result<String> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .context("artifact changed or cannot be opened safely; regenerate the declared image and rerun the check")?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file(),
        "artifact changed to a special file; regenerate the declared image and rerun the check"
    );
    ensure!(
        metadata.len() <= MAX_ARTIFACT_BYTES,
        "artifact exceeds its size limit; regenerate the declared image under 10 MiB and rerun the check"
    );
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 16384];
    let mut total = 0u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(
            total <= MAX_ARTIFACT_BYTES,
            "artifact grew beyond its size limit; stop the image writer, regenerate it under 10 MiB and rerun the check"
        );
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// Compare a recorded artifact against the file now on disk.
pub(super) fn evidence_status(path: &str, recorded: Option<&String>) -> EvidenceStatus {
    let Some(recorded) = recorded else {
        return EvidenceStatus::Unrecorded;
    };
    match file_hash(Path::new(path)) {
        Ok(current) if &current == recorded => EvidenceStatus::Verified,
        Ok(_) => EvidenceStatus::Tampered,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            EvidenceStatus::Missing
        }
        Err(_) => EvidenceStatus::Tampered,
    }
}

#[cfg(test)]
#[path = "files-tests.rs"]
mod lock_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_written_while_hashed_counts_as_changed_not_as_an_error() {
        let hash_of = |bytes: &[u8], length| {
            let mut hash = Sha256::new();
            hash_content(&mut hash, bytes, length).unwrap();
            format!("{:x}", hash.finalize())
        };
        let stable = hash_of(b"grown", 5);
        assert_ne!(hash_of(b"grown", 3), stable, "grew after it was listed");
        assert_ne!(hash_of(b"gro", 5), stable, "shrank after it was listed");
    }
}
