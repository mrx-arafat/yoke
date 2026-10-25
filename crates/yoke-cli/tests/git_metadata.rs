//! Repository metadata must never keep read-only displays waiting for a FIFO writer.

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn statusline_and_watch_skip_git_metadata_fifos_without_a_writer() {
    let mut blocked = Vec::new();
    for command in ["statusline", "watch"] {
        for name in [".git", ".git/HEAD"] {
            let home = tempfile::tempdir().unwrap();
            let project = tempfile::tempdir().unwrap();
            let path = project.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
            // SAFETY: path is a live, NUL-terminated path.
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            let mut child = Command::new(env!("CARGO_BIN_EXE_yoke"))
                .arg(command)
                .current_dir(project.path())
                .env("HOME", home.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            if command == "statusline" {
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(
                        serde_json::json!({"cwd": project.path()})
                            .to_string()
                            .as_bytes(),
                    )
                    .unwrap();
            } else {
                drop(child.stdin.take());
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if child.try_wait().unwrap().is_none() {
                child.kill().unwrap();
                blocked.push(format!("{command}: {name}"));
                child.wait().unwrap();
            } else {
                let output = child.wait_with_output().unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(!output.stdout.is_empty());
            }
            assert!(!home.path().join(".yoke").exists());
        }
    }
    assert!(blocked.is_empty(), "blocked on FIFOs: {blocked:?}");
}
