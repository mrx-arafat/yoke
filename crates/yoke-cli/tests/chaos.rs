//! Failure injection for the hook path: what the agent sees when the machine misbehaves.
//!
//! Every case runs the real `yoke` binary against its own temporary HOME. The rule under test:
//! observe mode never blocks the agent unless Yoke cannot know the mode, enforce mode fails
//! closed with a reason that names the way back, and nothing loops or leaks a process.

use std::fs;
use std::io::Write;
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use tempfile::TempDir;
use yoke_core::store::{DecisionFilter, Store};
use yoke_core::trust;

const DENY_ALL_BASH: &str = "mode='enforce'\n[[gate]]\nname='policy'\non='PreToolUse'\naction='deny'\nmatch_tool='Bash'\nmessage='blocked by policy'\n";

struct Fixture {
    _temp: TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        fs::create_dir_all(home.join(".yoke")).unwrap();
        fs::create_dir(&project).unwrap();
        Self {
            _temp: temp,
            home,
            project: project.canonicalize().unwrap(),
        }
    }

    fn store(&self) -> Store {
        Store::open(self.home.join(".yoke/state.db")).unwrap()
    }

    /// Acts as the human approving exact bytes; agents cannot reach this API.
    fn trust(&self, config: &str) {
        fs::write(self.project.join("yoke.toml"), config).unwrap();
        let store = self.store();
        trust::prepare_project(&store, &self.project)
            .unwrap()
            .commit(&store)
            .unwrap();
    }

    fn command(&self, event: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_yoke"));
        command
            .args(["run-hook", "--agent", "claude", event])
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env_remove("YOKE_OFF")
            .env_remove("YOKE_HOOK_DEADLINE_MS")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn hook(&self, event: &str, payload: &[u8]) -> Output {
        let mut child = self.command(event).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload).unwrap();
        child.wait_with_output().unwrap()
    }

    fn outcomes(&self) -> Vec<(String, String)> {
        self.store()
            .decisions(&DecisionFilter::default())
            .unwrap()
            .into_iter()
            .map(|row| (row.outcome, row.mode))
            .collect()
    }
}

fn bash(command: &str) -> Value {
    json!({"session_id":"chaos","tool_name":"Bash","tool_input":{"command":command}})
}

fn denial(output: &Output) -> Option<String> {
    let value: Value = serde_json::from_slice(&output.stdout).ok()?;
    value["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .or_else(|| value["reason"].as_str())
        .map(str::to_owned)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn observe_never_blocks_on_an_unusable_payload_cwd_and_enforce_keeps_its_policy() {
    // Observe mode: a cwd Yoke cannot use is recorded, never turned into a denial.
    let observe = Fixture::new();
    observe
        .trust("[[gate]]\nname='policy'\non='PreToolUse'\naction='deny'\nmessage='would block'\n");
    for cwd in ["relative/dir", "/nonexistent/chaos/dir"] {
        let mut payload = bash("ls");
        payload["cwd"] = json!(cwd);
        let output = observe.hook("PreToolUse", payload.to_string().as_bytes());
        assert_eq!(output.status.code(), Some(0), "{cwd}");
        assert!(output.stdout.is_empty(), "{cwd}: {}", stderr(&output));
    }
    assert!(observe.outcomes().iter().all(|(_, mode)| mode == "observe"));

    // Enforce mode: a deleted working directory is judged by the project above it, so the
    // agent is neither locked out of `cd` nor free of the gates.
    let enforce = Fixture::new();
    enforce.trust(DENY_ALL_BASH);
    let mut payload = bash("cd /tmp");
    payload["cwd"] = json!(enforce.project.join("deleted/deeper"));
    let output = enforce.hook("PreToolUse", payload.to_string().as_bytes());
    assert_eq!(denial(&output).as_deref(), Some("blocked by policy"));
}

#[test]
fn errors_say_whether_anything_was_blocked_and_a_bom_is_not_an_error() {
    let observe = Fixture::new();
    let output = observe.hook("PreToolUse", br#"{"session_id":"chaos","tool_name":"#);
    assert!(output.stdout.is_empty());
    let message = stderr(&output);
    assert!(message.contains("nothing was blocked"), "{message}");
    assert!(!message.contains("YOKE_OFF"), "{message}");

    let mut bom = b"\xef\xbb\xbf".to_vec();
    bom.extend(bash("ls").to_string().as_bytes());
    let output = observe.hook("PreToolUse", &bom);
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    assert_eq!(observe.outcomes()[0].0, "allow");

    let enforce = Fixture::new();
    enforce.trust(DENY_ALL_BASH);
    let output = enforce.hook("PreToolUse", b"not json");
    assert!(denial(&output).unwrap().contains("yoke pause"));
}

#[test]
fn unreadable_state_names_the_way_back_and_a_stop_is_blocked_only_once() {
    let f = Fixture::new();
    f.trust(DENY_ALL_BASH);
    fs::write(f.home.join(".yoke/state.db"), vec![0x5a; 4096]).unwrap();
    let _ = fs::remove_file(f.home.join(".yoke/state.db-wal"));
    let _ = fs::remove_file(f.home.join(".yoke/state.db-shm"));
    let reason = denial(&f.hook("PreToolUse", bash("ls").to_string().as_bytes())).unwrap();
    assert!(
        reason.contains("damaged") && reason.contains("move"),
        "{reason}"
    );
    assert!(!reason.contains("free disk space"), "{reason}");

    // Claude Code and Codex send stop_hook_active once a Stop hook already kept the agent
    // working; blocking again would loop the agent for as long as the state stays unreadable.
    let first = f.hook(
        "Stop",
        br#"{"session_id":"chaos","stop_hook_active":false}"#,
    );
    assert!(denial(&first).is_some(), "{}", stderr(&first));
    let second = f.hook("Stop", br#"{"session_id":"chaos","stop_hook_active":true}"#);
    assert_eq!(second.status.code(), Some(0));
    assert!(second.stdout.is_empty(), "{}", stderr(&second));
    assert!(stderr(&second).contains("damaged"), "{}", stderr(&second));

    // No HOME at all is also unknowable, and says how to set it.
    let mut command = f.command("PreToolUse");
    command.env_remove("HOME");
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(bash("ls").to_string().as_bytes())
        .unwrap();
    let reason = denial(&child.wait_with_output().unwrap()).unwrap();
    assert!(
        reason.contains("HOME") && !reason.contains("free disk space"),
        "{reason}"
    );
}

#[test]
fn an_oversized_payload_is_refused_with_advice_and_the_writer_is_not_cut_off() {
    let f = Fixture::new();
    f.trust("mode='enforce'\n");
    let mut child = f.command("PreToolUse").spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let payload = bash(&"a".repeat(3 * 1024 * 1024)).to_string();
    // A writer that the hook stops reading from would see a broken pipe here.
    stdin
        .write_all(payload.as_bytes())
        .expect("hook drains stdin");
    drop(stdin);
    let reason = denial(&child.wait_with_output().unwrap()).unwrap();
    assert!(
        reason.contains("1 MiB") && reason.contains("smaller"),
        "{reason}"
    );
}

#[test]
fn a_large_gate_input_is_marked_and_bounded_and_nul_is_scrubbed() {
    let f = Fixture::new();
    f.trust(r#"mode='enforce'
[[gate]]
name='lint'
on='PreToolUse'
match_contains='chaos-large-input-tail'
run='''test "$YOKE_TOOL_INPUT_TRUNCATED" = 1 && test "${#YOKE_TOOL_INPUT}" -le 32768 && test -z "${CHAOS_PAD+x}" && case "$YOKE_TOOL_INPUT" in *"[YOKE TOOL INPUT TRUNCATED]") exit 0;; *) exit 9;; esac'''
"#);
    let mut command = f.command("PreToolUse");
    // Undeclared inherited variables must not inflate the gate's environment.
    command.env("CHAOS_PAD", "x".repeat(16 * 1024));
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            bash(&format!("{}chaos-large-input-tail", "a".repeat(1_040_000)))
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let decisions = f.store().decisions(&DecisionFilter::default()).unwrap();
    assert_eq!(decisions[0].outcome, "allow");
    assert_eq!(decisions[0].gates[0]["name"], "lint");
    assert_eq!(decisions[0].gates[0]["result"], "pass");

    f.trust("mode='enforce'\n[[gate]]\nname='nul'\non='PreToolUse'\nrun='test \"$YOKE_TOOL_INPUT_TRUNCATED\" = 0 && test \"$YOKE_SESSION\" = ab && test \"$YOKE_TOOL_NAME\" = Bash'\n");
    let output = f.hook(
        "PreToolUse",
        br#"{"session_id":"a\u0000b","tool_name":"Ba\u0000sh","tool_input":{"command":"ls"}}"#,
    );
    assert!(output.stdout.is_empty(), "{}", stderr(&output));
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(f.outcomes()[0].0, "allow");
}

/// True while the process runs; a killed orphan nothing reaped is a zombie, which is not running.
fn process_exists(pid: i32) -> bool {
    // SAFETY: signal zero only probes whether the process exists.
    if unsafe { libc::kill(pid, 0) } == -1 {
        return std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH);
    }
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => !stat
            .rsplit_once(')')
            .is_some_and(|(_, rest)| rest.trim_start().starts_with(['Z', 'X'])),
        Err(_) => true,
    }
}

fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn read_pid(path: &Path) -> Option<i32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[test]
fn terminating_a_hook_stops_the_gate_it_started() {
    let f = Fixture::new();
    f.trust("mode='enforce'\n[[gate]]\nname='slow'\non='PreToolUse'\ntimeout=100\nrun='sleep 120 & echo $! > sleeper.pid; wait'\n");
    let pid_file = f.project.join("sleeper.pid");
    let mut hook: Child = f.command("PreToolUse").spawn().unwrap();
    hook.stdin
        .take()
        .unwrap()
        .write_all(bash("ls").to_string().as_bytes())
        .unwrap();
    wait_for("the gate to start", || read_pid(&pid_file).is_some());
    let sleeper = read_pid(&pid_file).unwrap();
    // SAFETY: the pid belongs to the hook this test just spawned.
    unsafe { libc::kill(hook.id() as i32, libc::SIGTERM) };
    hook.wait().unwrap();
    let stopped = {
        let deadline = Instant::now() + Duration::from_secs(10);
        while process_exists(sleeper) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        !process_exists(sleeper)
    };
    if !stopped {
        // SAFETY: cleanup of the sleeper this test started.
        unsafe { libc::kill(sleeper, libc::SIGKILL) };
    }
    assert!(stopped, "the gate's process outlived the terminated hook");
}

#[test]
fn an_emptied_state_file_is_reported_instead_of_passing_as_a_fresh_install() {
    let f = Fixture::new();
    f.trust(DENY_ALL_BASH);
    let db = f.home.join(".yoke/state.db");
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", db.display()));
    }
    // An old empty file: the trust approvals it held are gone, which the user must be told.
    let file = fs::File::create(&db).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(60))
        .unwrap();
    let output = f.hook("PreToolUse", bash("ls").to_string().as_bytes());
    assert!(
        stderr(&output).contains("state.db was empty"),
        "{}",
        stderr(&output)
    );
    let log = fs::read_to_string(f.home.join(".yoke/error.log")).unwrap();
    assert!(log.contains("run `yoke trust` again"), "{log}");
}

#[test]
fn a_stderr_nobody_reads_never_costs_the_agent_its_denial() {
    let f = Fixture::new();
    f.trust("mode='enforce'\n");
    // A pipe whose reader has gone: every write to it fails with EPIPE.
    let mut fds = [0; 2];
    // SAFETY: plain pipe(2) on a local array; both descriptors are closed or owned below.
    let (reader, writer) = unsafe {
        assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
        (fds[0], fs::File::from_raw_fd(fds[1]))
    };
    // SAFETY: closing the read end this test just opened.
    unsafe { libc::close(reader) };
    let mut command = f.command("PreToolUse");
    command.stderr(Stdio::from(writer));
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"not json").unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(denial(&output).unwrap().contains("yoke pause"));
}

// The last-known-mode record: what a hook may assume when it cannot read its state.

fn cache_file(f: &Fixture) -> PathBuf {
    f.home.join(".yoke/mode-cache")
}

fn corrupt_state(f: &Fixture) {
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(f.home.join(format!(".yoke/state.db{suffix}")));
    }
    fs::write(f.home.join(".yoke/state.db"), vec![0x5a; 4096]).unwrap();
}

#[test]
fn a_project_last_seen_in_observe_mode_is_allowed_through_corrupt_state_with_a_loud_warning() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let f = Fixture::new();
    // An ordinary hook run is what records the mode; nothing is written by hand.
    assert!(f
        .hook("PreToolUse", bash("ls").to_string().as_bytes())
        .stdout
        .is_empty());
    let record = fs::metadata(cache_file(&f)).unwrap();
    assert_eq!(record.permissions().mode() & 0o777, 0o600);
    // The mode did not change, so the next hook leaves the file alone.
    f.hook("PreToolUse", bash("ls").to_string().as_bytes());
    assert_eq!(fs::metadata(cache_file(&f)).unwrap().ino(), record.ino());

    corrupt_state(&f);
    for event in ["PreToolUse", "Stop"] {
        let output = f.hook(event, bash("ls").to_string().as_bytes());
        assert_eq!(output.status.code(), Some(0), "{event}");
        assert!(
            output.stdout.is_empty(),
            "{event}: observe promised to block nothing"
        );
        let message = stderr(&output);
        assert!(
            message.contains("last known mode") && message.contains("observe"),
            "{message}"
        );
        assert!(
            message.contains("damaged"),
            "the way back is still named: {message}"
        );
    }
    let log = fs::read_to_string(f.home.join(".yoke/error.log")).unwrap();
    assert!(log.contains("last known mode"), "{log}");
}

#[test]
fn a_project_last_seen_in_enforce_mode_stays_denied_when_state_is_corrupt() {
    let f = Fixture::new();
    f.trust(DENY_ALL_BASH);
    let output = f.hook("PreToolUse", bash("ls").to_string().as_bytes());
    assert_eq!(denial(&output).as_deref(), Some("blocked by policy"));
    corrupt_state(&f);
    let reason = denial(&f.hook("PreToolUse", bash("ls").to_string().as_bytes())).unwrap();
    assert!(
        reason.contains("damaged") && reason.contains("move"),
        "{reason}"
    );
    assert!(
        reason.contains("last known mode for this project is enforce"),
        "{reason}"
    );
}

#[test]
fn corrupt_state_with_no_record_of_the_mode_is_denied_with_the_way_back() {
    let f = Fixture::new();
    corrupt_state(&f);
    assert!(!cache_file(&f).exists());
    let reason = denial(&f.hook("PreToolUse", bash("ls").to_string().as_bytes())).unwrap();
    assert!(
        reason.contains("damaged") && reason.contains("move"),
        "{reason}"
    );
    // Another project's record says nothing about this one.
    let other = Fixture::new();
    other.hook("PreToolUse", bash("ls").to_string().as_bytes());
    fs::copy(cache_file(&other), cache_file(&f)).unwrap();
    let reason = denial(&f.hook("PreToolUse", bash("ls").to_string().as_bytes())).unwrap();
    assert!(reason.contains("damaged"), "{reason}");
}

#[test]
fn a_record_of_the_mode_that_fails_its_check_is_ignored() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.trust(DENY_ALL_BASH);
    f.hook("PreToolUse", bash("ls").to_string().as_bytes());
    let honest = fs::read_to_string(cache_file(&f)).unwrap();
    assert!(honest.contains(" enforce "), "{honest}");
    corrupt_state(&f);
    // Flipping enforce to observe by hand is the edit the hash exists to catch.
    fs::write(cache_file(&f), honest.replace(" enforce ", " observe ")).unwrap();
    let edited = f.hook("PreToolUse", bash("ls").to_string().as_bytes());
    let reason = denial(&edited).expect("an edited record must not allow the call");
    assert!(reason.contains("cannot be used"), "{reason}");
    // The same for a record nobody can read, and one anybody can write.
    fs::write(cache_file(&f), honest.replace(" enforce ", " observe ")).unwrap();
    fs::set_permissions(cache_file(&f), fs::Permissions::from_mode(0o000)).unwrap();
    assert!(denial(&f.hook("PreToolUse", bash("ls").to_string().as_bytes())).is_some());
    fs::set_permissions(cache_file(&f), fs::Permissions::from_mode(0o666)).unwrap();
    assert!(denial(&f.hook("PreToolUse", bash("ls").to_string().as_bytes())).is_some());
}

#[test]
fn an_agent_cannot_rewrite_or_remove_the_record_of_the_mode() {
    let f = Fixture::new();
    f.trust("mode='enforce'\n");
    f.hook("PreToolUse", bash("ls").to_string().as_bytes());
    let target = f.home.join(".yoke/mode-cache");
    let shell = [
        format!("echo observe > {}", target.display()),
        format!("rm {}", target.display()),
        format!("printf x | tee {}", target.display()),
        "echo x >> \"$HOME/.yoke/mode-cache\"".to_owned(),
        "mv /tmp/forged \"$HOME/.yoke/mode-cache\"".to_owned(),
    ];
    for command in shell {
        let reason = denial(&f.hook("PreToolUse", bash(&command).to_string().as_bytes()));
        assert!(reason.is_some(), "the guard let this through: {command}");
    }
    let write = json!({"session_id":"chaos","tool_name":"Write","tool_input":{"file_path":target,"content":"x"}});
    assert!(denial(&f.hook("PreToolUse", write.to_string().as_bytes())).is_some());
    assert!(target.exists(), "the record is still there");
}
