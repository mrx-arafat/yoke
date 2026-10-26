//! The live view's terminal: raw keyboard input and the alternate screen, restored on every
//! exit path (return, error, panic and the termination signals).

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Set by SIGINT, SIGTERM and SIGHUP; the loop then returns and the guard restores the terminal.
static STOP: AtomicBool = AtomicBool::new(false);
/// Settings to put back, taken by whichever restore runs first.
static SAVED: Mutex<Option<libc::termios>> = Mutex::new(None);

const ENTER: &str = "\x1b[?1049h\x1b[?25l";
const LEAVE: &str = "\x1b[0m\x1b[?25h\x1b[?1049l";

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

pub(super) fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// Put the saved settings back and leave the alternate screen; later calls do nothing.
fn restore() {
    let saved = SAVED.lock().map_or(None, |mut saved| saved.take());
    if let Some(saved) = saved {
        // SAFETY: restores settings read from this same descriptor by `Raw::enter`.
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &saved);
        }
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(LEAVE.as_bytes());
        let _ = stdout.flush();
    }
}

/// Raw input and the alternate screen for as long as it lives.
pub(super) struct Raw;

impl Raw {
    pub(super) fn enter() -> std::io::Result<Self> {
        // SAFETY: tcgetattr fills the zeroed struct; tcsetattr reads the modified copy.
        let mut settings: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut settings) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let saved = settings;
        // Keys arrive one at a time without echo; Ctrl-C arrives as a byte so the loop quits
        // through the same path as `q`. Output processing stays on.
        settings.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
        settings.c_iflag &= !(libc::IXON | libc::ICRNL);
        settings.c_cc[libc::VMIN] = 0;
        settings.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &settings) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if let Ok(mut slot) = SAVED.lock() {
            *slot = Some(saved);
        }
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
            unsafe {
                libc::signal(signal, on_signal as extern "C" fn(libc::c_int) as usize);
            }
        }
        let raw = Self;
        let mut stdout = std::io::stdout();
        stdout.write_all(ENTER.as_bytes())?;
        stdout.flush()?;
        Ok(raw)
    }

    /// Bytes typed within `timeout`, empty when none; a signal ends the wait early.
    pub(super) fn keys(&self, timeout: Duration) -> Vec<u8> {
        let mut poll = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);
        // SAFETY: one valid pollfd; read writes at most the buffer's length.
        if unsafe { libc::poll(&mut poll, 1, millis) } <= 0 {
            return Vec::new();
        }
        let mut buffer = [0u8; 64];
        let read = unsafe { libc::read(libc::STDIN_FILENO, buffer.as_mut_ptr().cast(), 64) };
        buffer[..usize::try_from(read).unwrap_or(0)].to_vec()
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        restore();
    }
}

/// Columns and rows of the terminal on stdout, when it reports a usable size.
pub(super) fn size() -> Option<(usize, usize)> {
    // SAFETY: TIOCGWINSZ fills the zeroed winsize.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } == 0;
    (ok && size.ws_col > 0 && size.ws_row > 0)
        .then(|| (usize::from(size.ws_col), usize::from(size.ws_row)))
}

/// Paint lines from the top left without clearing first, so a redraw does not flicker: every
/// line clears only what is left of its old text, and the rest of the screen goes at the end.
pub(super) fn paint(lines: &[String]) -> std::io::Result<()> {
    let mut frame = String::from("\x1b[H");
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            frame.push_str("\r\n");
        }
        frame.push_str(line);
        frame.push_str("\x1b[0m\x1b[K");
    }
    frame.push_str("\x1b[J");
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(frame.as_bytes())?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::net::UnixStream;

    #[test]
    fn raw_modes_restore_after_output_error() {
        const CHILD: &str = "YOKE_TEST_RAW_OUTPUT_FAILURE";
        if std::env::var_os(CHILD).is_none() {
            // Descriptor and signal changes stay in a single-test child, never the test runner
            // or the person's terminal. The child creates its own disposable PTY.
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "watch::term::tests::raw_modes_restore_after_output_error",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed; 0 failed"));
            return;
        }
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: openpty writes two owned descriptors to the live locals; null termios and
        // winsize pointers request platform defaults. Only this isolated child changes fds.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let (_master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        let saved_stdout = unsafe { libc::dup(libc::STDOUT_FILENO) };
        assert!(saved_stdout >= 0);
        let saved_stdout = unsafe { OwnedFd::from_raw_fd(saved_stdout) };
        let (output, reader) = UnixStream::pair().unwrap();
        drop(reader);
        assert_eq!(
            unsafe { libc::dup2(slave.as_raw_fd(), libc::STDIN_FILENO) },
            libc::STDIN_FILENO
        );
        assert_eq!(
            unsafe { libc::dup2(output.as_raw_fd(), libc::STDOUT_FILENO) },
            libc::STDOUT_FILENO
        );
        let settings = || {
            let mut value: libc::termios = unsafe { std::mem::zeroed() };
            assert_eq!(
                unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut value) },
                0
            );
            value
        };
        let before = settings();
        let result = Raw::enter();
        let after = settings();
        assert_eq!(
            unsafe { libc::dup2(saved_stdout.as_raw_fd(), libc::STDOUT_FILENO) },
            libc::STDOUT_FILENO
        );
        assert!(
            result.is_err(),
            "a closed output peer must refuse the screen write"
        );
        // macOS may set PENDIN while applying termios; compare the modes we changed.
        let local = libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN;
        let input = libc::IXON | libc::ICRNL;
        assert_eq!(after.c_lflag & local, before.c_lflag & local);
        assert_eq!(after.c_iflag & input, before.c_iflag & input);
        assert_eq!(after.c_cc, before.c_cc);
    }
}
