//! Process construction at the boundary with project-controlled working directories.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, ensure, Result};

/// Kernel process identity used to correlate a recorded hook parent with a tool's ancestry.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Identity {
    pub pid: i32,
    pub parent_pid: i32,
    pub name: String,
    pub started: String,
}

/// Read process identity without spawning a PATH-resolved helper.
#[cfg(target_os = "macos")]
pub fn identity(pid: i32) -> Option<Identity> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: proc_pidinfo receives a live buffer of exactly the requested structure size.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    };
    if written != size as i32 {
        return None;
    }
    // SAFETY: a successful exact-size proc_pidinfo call initialized every field.
    let info = unsafe { info.assume_init() };
    let name = info
        .pbi_name
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect::<Vec<_>>();
    Some(Identity {
        pid,
        parent_pid: info.pbi_ppid as i32,
        name: String::from_utf8_lossy(&name).into_owned(),
        started: format!("{}:{}", info.pbi_start_tvsec, info.pbi_start_tvusec),
    })
}

#[cfg(target_os = "linux")]
pub fn identity(pid: i32) -> Option<Identity> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let fields = stat[close + 1..].split_whitespace().collect::<Vec<_>>();
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    Some(Identity {
        pid,
        parent_pid: fields.get(1)?.parse().ok()?,
        name: stat[open + 1..close].to_owned(),
        started: format!("{}:{}", boot.trim(), fields.get(19)?),
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn identity(_: i32) -> Option<Identity> {
    None
}

/// Bounded current ancestry; a missing identity makes correlation unavailable, not guessed.
pub fn ancestors() -> Vec<Identity> {
    let mut result = Vec::new();
    // SAFETY: getppid only reads the current process's kernel parent PID.
    let mut pid = unsafe { libc::getppid() };
    for _ in 0..32 {
        let Some(found) = identity(pid) else { break };
        let parent = found.parent_pid;
        result.push(found);
        if parent <= 1 || parent == pid {
            break;
        }
        pid = parent;
    }
    result
}

/// Find an executable without resolving relative PATH entries in a project directory.
pub fn executable(name: &str) -> Result<PathBuf> {
    resolve(name, std::env::var_os("PATH").as_deref())
}

fn resolve(name: &str, path: Option<&OsStr>) -> Result<PathBuf> {
    ensure!(
        !name.is_empty() && !name.contains('/'),
        "executable lookup requires a plain program name"
    );
    if let Some(path) = path {
        for directory in std::env::split_paths(path).filter(|path| path.is_absolute()) {
            let candidate = directory.join(name);
            if let Ok(path) = candidate.canonicalize() {
                if path
                    .metadata()
                    .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
                {
                    return Ok(path);
                }
            }
        }
    }
    bail!("{name} is not executable in an absolute PATH directory; install it and add its directory to PATH")
}

/// Git must not run repository hooks or inherit caller-controlled repository/config overrides.
pub fn git() -> Result<Command> {
    let mut command = Command::new(executable("git")?);
    for (name, _) in
        std::env::vars_os().filter(|(name, _)| name.as_encoded_bytes().starts_with(b"GIT_"))
    {
        command.env_remove(name);
    }
    command.args([
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.hooksPath=/dev/null",
        "--no-optional-locks",
    ]);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_uses_absolute_path_and_disables_repository_execution() {
        let command = git().unwrap();
        assert!(std::path::Path::new(command.get_program()).is_absolute());
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.hooksPath=/dev/null",
                "--no-optional-locks"
            ]
        );
        for (name, _) in
            std::env::vars_os().filter(|(name, _)| name.as_encoded_bytes().starts_with(b"GIT_"))
        {
            assert!(command
                .get_envs()
                .any(|(key, value)| key == name && value.is_none()));
        }
    }

    #[test]
    fn executable_lookup_ignores_relative_and_empty_path_entries() {
        assert!(resolve("git", Some(OsStr::new(":.:bin"))).is_err());
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("tool");
        std::fs::write(&file, "#!/bin/sh\n").unwrap();
        assert!(resolve("tool", Some(directory.path().as_os_str())).is_err());
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            resolve("tool", Some(directory.path().as_os_str())).unwrap(),
            file.canonicalize().unwrap()
        );
    }
}
