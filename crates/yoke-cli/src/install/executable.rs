use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};

pub(super) fn canonical(path: &Path) -> Result<PathBuf> {
    let canonical = path.canonicalize().with_context(|| {
        format!(
            "hook executable {} is unavailable; install a working yoke before registering hooks",
            path.display()
        )
    })?;
    let metadata = canonical.metadata()?;
    ensure!(
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0,
        "hook executable {} must be a regular executable file; install a working yoke before registering hooks",
        canonical.display()
    );
    Ok(canonical)
}

pub(super) fn warnings(path: &Path) -> Vec<String> {
    let (mut problems, guidance) = diagnostics(path);
    problems.extend(guidance);
    problems
}

pub(super) fn diagnostics(path: &Path) -> (Vec<String>, Vec<String>) {
    let mut problems = Vec::new();
    let mut guidance = Vec::new();
    if path.components().any(|part| part.as_os_str() == "target") {
        problems.push(format!("hook executable {} is in a target build directory; install a stable copy outside target, then uninstall and reinstall the hooks", path.display()));
    }
    // SAFETY: geteuid only reads the process's effective user ID.
    let uid = unsafe { libc::geteuid() };
    let mut child: Option<&Path> = None;
    for ancestor in path.ancestors() {
        match ancestor.metadata() {
            Ok(meta) => {
                let mode = meta.permissions().mode();
                let protected_entry = meta.is_dir()
                    && mode & 0o1000 != 0
                    && child.is_some_and(|child| {
                        child.metadata().is_ok_and(|meta| meta.uid() == uid || meta.uid() == 0)
                    });
                let location = crate::commands::quote(&ancestor.display().to_string());
                if mode & 0o002 != 0 {
                    if !protected_entry {
                        problems.push(format!("hook executable location {} is writable by others without a protected sticky entry; use a private installation directory or remove other write access with `chmod o-w {location}` before reinstalling hooks", ancestor.display()));
                    }
                } else if mode & 0o020 != 0 {
                    guidance.push(format!("hook executable location {} is group-writable; if shared write access is unnecessary, remove it with `chmod g-w {location}` or install a private copy", ancestor.display()));
                }
            }
            Err(error) => problems.push(format!("cannot inspect hook executable location {}: {error}; check its permissions before reinstalling hooks", ancestor.display())),
        }
        child = Some(ancestor);
    }
    (problems, guidance)
}
