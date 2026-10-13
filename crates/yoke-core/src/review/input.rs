use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path};
use std::process::Command;
use std::time::Duration;

use anyhow::{ensure, Context, Result};

use crate::engine::execute_bounded;

const INPUT_CAP: usize = 1024 * 1024;

pub(super) fn collect(project: &Path) -> Result<String> {
    let cwd = project.canonicalize().context("resolve review project")?;
    let root = git(&cwd, &["rev-parse", "--show-toplevel"])?;
    let root = std::str::from_utf8(&root).context("review repository root is not UTF-8")?;
    let project = Path::new(root.strip_suffix('\n').unwrap_or(root)).canonicalize()?;
    let diff = git(
        &project,
        &["diff", "--no-ext-diff", "--no-textconv", "HEAD", "--"],
    )?;
    let diff = String::from_utf8(diff).context("review diff is not UTF-8")?;
    ensure!(
        !diff
            .lines()
            .any(|line| line.starts_with("Binary files ") || line == "GIT binary patch"),
        "binary tracked changes cannot be reviewed as text"
    );
    let paths = git(
        &project,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    append_untracked(&project, diff, &paths)
}

fn git(project: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(project)
        .env("GIT_OPTIONAL_LOCKS", "0");
    let out = execute_bounded(&mut command, &[], Duration::from_secs(30), INPUT_CAP)
        .context("read review changes")?;
    ensure!(
        out.success && !out.stdout_truncated,
        "review change discovery failed, timed out, or exceeded 1 MiB: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(out.stdout)
}

pub(super) fn append_untracked(project: &Path, mut input: String, paths: &[u8]) -> Result<String> {
    let project = project.canonicalize()?;
    for name in paths
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name = std::str::from_utf8(name).context("untracked path is not UTF-8")?;
        let path = Path::new(name);
        ensure!(
            path.components()
                .all(|component| matches!(component, Component::Normal(_))),
            "untracked review path must be project-relative"
        );
        let full = project.join(path);
        let parent = full
            .parent()
            .context("untracked path missing parent")?
            .canonicalize()?;
        ensure!(
            parent.starts_with(&project),
            "untracked review path escapes project: {name}"
        );
        let metadata = fs::symlink_metadata(&full)?;
        let item = if metadata.file_type().is_symlink() {
            let target = fs::read_link(&full)?;
            serde_json::json!({"path": name, "kind": "symlink", "target": target.to_str().context("symlink target is not UTF-8")?})
        } else {
            ensure!(
                metadata.is_file(),
                "unsupported untracked file type: {name}"
            );
            let remaining = INPUT_CAP.saturating_sub(input.len());
            ensure!(
                metadata.len() <= remaining as u64,
                "review changes exceed 1 MiB at {name}"
            );
            let mut bytes = Vec::new();
            File::options()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&full)?
                .take(remaining as u64 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() <= remaining,
                "review changes exceed 1 MiB at {name}"
            );
            ensure!(
                !bytes.contains(&0),
                "binary untracked file cannot be reviewed as text: {name}"
            );
            let content = String::from_utf8(bytes)
                .with_context(|| format!("untracked file is not UTF-8: {name}"))?;
            serde_json::json!({"path": name, "kind": "file", "executable": metadata.permissions().mode() & 0o111 != 0, "content": content})
        };
        input.push_str("\nUNTRACKED FILE DATA: ");
        input.push_str(&serde_json::to_string(&item)?);
        input.push('\n');
        ensure!(
            input.len() <= INPUT_CAP,
            "review changes including metadata exceed 1 MiB"
        );
    }
    Ok(input)
}
