use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};

use super::{PlanHeader, PlanRecord};
use crate::engine::execute_bounded;

pub(super) fn project(path: &Path) -> Result<PathBuf> {
    let root = path.canonicalize().context("resolving project root")?;
    ensure!(root.is_dir(), "project root must be a directory");
    ensure!(root.to_str().is_some(), "project root must be UTF-8");
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
        "plans directory must be a relative path without traversal"
    );
    let directory = root.join(directory);
    let ancestor = directory
        .ancestors()
        .find(|path| path.exists())
        .context("plans directory has no existing ancestor")?;
    ensure!(
        ancestor.canonicalize()?.starts_with(root),
        "plans directory escapes project"
    );
    fs::create_dir_all(&directory)?;
    let directory = directory.canonicalize()?;
    ensure!(
        directory.starts_with(root),
        "plans directory escapes project"
    );
    Ok(directory.join(format!("{slug}.md")))
}

pub(super) fn read(plan: &PlanRecord) -> Result<String> {
    let root = project(Path::new(&plan.project))?;
    let path = Path::new(&plan.file)
        .canonicalize()
        .context("resolving plan file")?;
    ensure!(path.starts_with(&root), "plan file escapes project");
    let metadata = fs::metadata(&path)?;
    ensure!(
        metadata.is_file() && metadata.len() <= 1024 * 1024,
        "plan must be a regular file of at most 1 MiB"
    );
    fs::read_to_string(&path).context("reading plan")
}

pub(super) fn parse(content: &str) -> Result<(String, PlanHeader)> {
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
    Ok((raw, header))
}

pub(super) fn approved(plan: &PlanRecord) -> Result<PlanHeader> {
    let frozen = plan
        .approved_header
        .as_ref()
        .context("plan has not been approved")?;
    let (raw, header) = parse(&read(plan)?)?;
    ensure!(
        &raw == frozen,
        "plan header changed; human re-approval required"
    );
    Ok(header)
}

pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = execute_bounded(
        Command::new("git").current_dir(root).args(args),
        &[],
        Duration::from_secs(30),
        16 * 1024 * 1024,
    )?;
    ensure!(
        output.success && !output.timed_out && !output.stdout_truncated,
        "git fingerprint command failed or exceeded its bound: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

/// Fingerprint tracked changes and every untracked, nonignored file in the repository.
pub fn fingerprint(project_path: &Path) -> Result<String> {
    let cwd = project(project_path)?;
    let top = git(&cwd, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(OsString::from_vec(
        top.strip_suffix(b"\n").unwrap_or(&top).to_vec(),
    ));
    let mut hash = Sha256::new();
    field(&mut hash, &git(&root, &["rev-parse", "--verify", "HEAD"])?);
    field(
        &mut hash,
        &git(
            &root,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--binary",
                "HEAD",
                "--",
            ],
        )?,
    );
    let untracked = git(&root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut paths: Vec<_> = untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .collect();
    paths.sort_unstable();
    for raw in paths {
        let path = PathBuf::from(OsString::from_vec(raw.to_vec()));
        ensure!(relative(&path), "invalid untracked path returned by git");
        let full = root.join(&path);
        let metadata = fs::symlink_metadata(&full)?;
        field(&mut hash, raw);
        if metadata.file_type().is_symlink() {
            field(&mut hash, b"symlink");
            field(&mut hash, fs::read_link(&full)?.as_os_str().as_bytes());
        } else {
            ensure!(
                metadata.is_file(),
                "untracked entry is not a regular file: {}",
                path.display()
            );
            field(&mut hash, b"file");
            use std::os::unix::fs::PermissionsExt;
            field(
                &mut hash,
                &(metadata.permissions().mode() & 0o111).to_le_bytes(),
            );
            hash.update(metadata.len().to_le_bytes());
            let mut file = fs::File::open(&full)?;
            let mut buffer = [0u8; 16384];
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hash.update(&buffer[..count]);
            }
        }
    }
    Ok(format!("{:x}", hash.finalize()))
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

pub(super) fn artifacts(root: &Path, declared: &[String]) -> Result<Vec<String>> {
    declared
        .iter()
        .map(|artifact| {
            let path = root
                .join(artifact)
                .canonicalize()
                .with_context(|| format!("missing declared artifact {artifact}"))?;
            ensure!(
                path.starts_with(root) && image_extension(&path),
                "artifact escapes project or is not an image"
            );
            let metadata = fs::metadata(&path)?;
            ensure!(
                metadata.is_file() && metadata.len() <= 10 * 1024 * 1024,
                "artifact must be a regular image of at most 10 MiB"
            );
            Ok(path
                .to_str()
                .context("artifact path must be UTF-8")?
                .to_owned())
        })
        .collect()
}
