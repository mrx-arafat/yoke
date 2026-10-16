use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use yoke_core::{memory, store::Store, trust};

fn hook(home: &Path, project: &Path, bin: &Path, event: &str, payload: &Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_yoke"))
        .args(["run-hook", event, "--agent", "claude"])
        .env("HOME", home)
        .env("PATH", bin)
        .env_remove("YOKE_OFF")
        .current_dir(project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn jobs(home: &Path, project: &Path) -> Vec<Value> {
    let output = Command::new(env!("CARGO_BIN_EXE_yoke"))
        .args(["jobs", "--all", "--json"])
        .env("HOME", home)
        .current_dir(project)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice::<Value>(&output.stdout)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn session_start_digests_facts_of_a_killed_session_exactly_once() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    let bin = temp.path().join("bin");
    fs::create_dir_all(home.join(".yoke")).unwrap();
    fs::create_dir(&project).unwrap();
    fs::create_dir(&bin).unwrap();
    let project = project.canonicalize().unwrap();
    fs::write(
        project.join("yoke.toml"),
        "mode='enforce'\n[memory]\ndigest=true\ndigest_agent='codex'\n",
    )
    .unwrap();
    let store = Store::open(home.join(".yoke/state.db")).unwrap();
    trust::prepare_project(&store, &project)
        .unwrap()
        .commit(&store)
        .unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    // Claim the update interval so session start never starts network work.
    store
        .set_kv("update.last_attempt", &now.to_string())
        .unwrap();
    // The killed session edited a file an hour ago; its SessionEnd never arrived.
    hook(
        &home,
        &project,
        &bin,
        "PostToolUse",
        &json!({"session_id":"killed","tool_name":"Edit","tool_input":{"file_path":"src/lib.rs"}}),
    );
    let raw = rusqlite::Connection::open(home.join(".yoke/state.db")).unwrap();
    raw.execute("UPDATE facts SET ts=ts-3600", []).unwrap();
    raw.execute("UPDATE decisions SET ts=ts-3600", []).unwrap();
    let fact: i64 = raw
        .query_row(
            "SELECT id FROM facts WHERE session_id='killed' AND digested=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let script = bin.join("codex");
    fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' \"$YOKE_OFF\" >> \"$HOME/digest-calls\"\n/bin/cat > /dev/null\nprintf '%s' '{{\"memories\":[{{\"text\":\"Fixture digest\",\"fact_ids\":[{fact}]}}]}}'\n")).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    // A later session starts; nothing else ever refers to the killed session again.
    hook(
        &home,
        &project,
        &bin,
        "SessionStart",
        &json!({"session_id":"next"}),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let jobs = jobs(&home, &project);
        if jobs.len() == 1 && jobs[0]["state"] == "succeeded" {
            assert_eq!(jobs[0]["kind"], "memory_digest");
            assert_eq!(jobs[0]["attempts"], 1);
            assert_eq!(
                serde_json::from_str::<Value>(jobs[0]["payload"].as_str().unwrap()).unwrap(),
                json!({"session":"killed","transcript":null})
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "orphan digest did not run: jobs {jobs:?}, error log {:?}",
            fs::read_to_string(home.join(".yoke/error.log"))
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let notes = memory::recall(&store, project.to_str().unwrap(), "Fixture", 10).unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].source, "digest");
    assert!(
        !memory::digest_due(&store, "killed", project.to_str().unwrap(), now + 10_000, 0).unwrap()
    );
    // Another session start finds nothing pending and never digests the same facts again.
    hook(
        &home,
        &project,
        &bin,
        "SessionStart",
        &json!({"session_id":"later"}),
    );
    assert_eq!(jobs(&home, &project).len(), 1);
    assert_eq!(
        fs::read_to_string(home.join("digest-calls")).unwrap(),
        "1\n"
    );
}
