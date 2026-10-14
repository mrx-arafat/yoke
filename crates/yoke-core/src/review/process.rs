use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Bounded byte tails and execution evidence from one child process.
#[derive(Debug)]
pub struct ProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub timed_out: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub ms: u64,
}

/// Process groups started here that may still be running, so a process that has to exit
/// early can stop them first instead of leaving them orphaned.
static RUNNING_GROUPS: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// Kill every process group `execute_bounded` started that has not been reaped yet.
pub fn kill_running_groups() {
    let mut groups = RUNNING_GROUPS
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    for group in groups.drain(..) {
        // SAFETY: each id is a private process group created by `execute_bounded`.
        unsafe { libc::kill(-group, libc::SIGKILL) };
    }
}

struct ChildGroup(Child, bool);

impl ChildGroup {
    fn terminate(&mut self) -> io::Result<()> {
        if self.1 {
            return Ok(());
        }
        let group = self.0.id() as i32;
        RUNNING_GROUPS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|running| *running != group);
        // SAFETY: spawn creates a private process group whose id is the child's pid.
        if unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL) } == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        if self.0.try_wait()?.is_none() {
            self.0.kill()?;
        }
        self.1 = true;
        Ok(())
    }
}

impl Drop for ChildGroup {
    fn drop(&mut self) {
        // Fallback for earlier I/O errors; normal-path cleanup propagates errors.
        let _ = self.terminate();
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    // SAFETY: the borrowed descriptor stays open during both fcntl operations.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags == -1
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn drain(
    pipe: &mut impl Read,
    tail: &mut Vec<u8>,
    cap: usize,
    truncated: &mut bool,
) -> io::Result<()> {
    let mut buffer = [0; 8192];
    // Bound each poll so continuous output cannot starve input or deadline checks.
    for _ in 0..8 {
        match pipe.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                tail.extend_from_slice(&buffer[..n]);
                if tail.len() > cap {
                    tail.drain(..tail.len() - cap);
                    *truncated = true;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Execute with concurrent nonblocking input/output and a private process group.
/// Callers configure args, cwd, and environment; stdin/stdout/stderr are replaced.
pub fn execute_bounded(
    command: &mut Command,
    input: &[u8],
    timeout: Duration,
    output_cap: usize,
) -> io::Result<ProcessOutput> {
    if timeout.is_zero() || output_cap == 0 || output_cap > 16 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "positive timeout and output cap up to 16 MiB required",
        ));
    }
    let start = Instant::now();
    let mut child = ChildGroup(
        command
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?,
        false,
    );
    RUNNING_GROUPS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(child.0.id() as i32);
    let mut stdout = child.0.stdout.take().expect("piped stdout");
    let mut stderr = child.0.stderr.take().expect("piped stderr");
    let mut stdin = child.0.stdin.take();
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    nonblocking(stdin.as_ref().expect("piped stdin"))?;
    let mut written = 0;
    let mut out = ProcessOutput {
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit_code: None,
        success: false,
        timed_out: false,
        stdout_truncated: false,
        stderr_truncated: false,
        ms: 0,
    };
    loop {
        drain(
            &mut stdout,
            &mut out.stdout,
            output_cap,
            &mut out.stdout_truncated,
        )?;
        drain(
            &mut stderr,
            &mut out.stderr,
            output_cap,
            &mut out.stderr_truncated,
        )?;
        if written == input.len() {
            stdin.take();
        }
        if let Some(pipe) = stdin.as_mut() {
            let end = input.len().min(written.saturating_add(65536));
            match pipe.write(&input[written..end]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "child stdin stopped accepting input",
                    ))
                }
                Ok(n) => written += n,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e),
            }
        }
        if let Some(status) = child.0.try_wait()? {
            child.terminate()?;
            drain(
                &mut stdout,
                &mut out.stdout,
                output_cap,
                &mut out.stdout_truncated,
            )?;
            drain(
                &mut stderr,
                &mut out.stderr,
                output_cap,
                &mut out.stderr_truncated,
            )?;
            out.exit_code = status.code();
            out.success = status.success() && written == input.len();
            break;
        }
        if start.elapsed() >= timeout {
            child.terminate()?;
            child.0.wait()?;
            out.timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    out.ms = start.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_stdin_without_deadlock_and_reports_truncation() {
        let input = vec![b'x'; 256 * 1024];
        let out = execute_bounded(
            Command::new("sh").args(["-c", "cat; printf END; printf diagnostic >&2"]),
            &input,
            Duration::from_secs(3),
            1024,
        )
        .unwrap();
        assert!(out.success);
        assert_eq!(out.stdout.len(), 1024);
        assert!(out.stdout.ends_with(b"END"));
        assert!(out.stdout_truncated);
        assert_eq!(out.stderr, b"diagnostic");
        assert!(!out.stderr_truncated);
    }

    #[test]
    fn times_out_a_child_that_never_reads_input() {
        let start = Instant::now();
        let out = execute_bounded(
            Command::new("sh").args(["-c", "sleep 30"]),
            &vec![b'x'; 256 * 1024],
            Duration::from_millis(100),
            1024,
        )
        .unwrap();
        assert!(out.timed_out);
        assert!(!out.success);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
