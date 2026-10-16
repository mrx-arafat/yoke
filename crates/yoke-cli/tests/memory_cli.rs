use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn run(home: &Path, project: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_yoke"))
        .args(args)
        .env("HOME", home)
        .current_dir(project)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn out(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn err(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn fresh() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let temp = tempfile::tempdir().unwrap();
    let (home, a, b) = (
        temp.path().join("home"),
        temp.path().join("a"),
        temp.path().join("b"),
    );
    for path in [&home, &a, &b] {
        fs::create_dir(path).unwrap();
    }
    (temp, home, a, b)
}

#[test]
fn memory_ids_stay_in_their_project_and_the_cli_says_when_nothing_is_injected() {
    let (_temp, home, a, b) = fresh();
    let first = run(&home, &b, &["remember", "Keep the migration reversible"]);
    assert!(first.status.success());
    let note: Value = serde_json::from_slice(&first.stdout).unwrap();
    let id = note["id"].to_string();
    assert!(err(&first).contains("injects no notes while mode is observe"));
    // The hint comes once per project, with its first note.
    let second = run(&home, &b, &["remember", "Second note"]);
    assert!(err(&second).is_empty(), "{}", err(&second));

    // Another project's id is not found, by every id command, and nothing there changes.
    for args in [
        ["forget", id.as_str()].as_slice(),
        ["share", id.as_str()].as_slice(),
        ["memory", "pin", id.as_str()].as_slice(),
    ] {
        let denied = run(&home, &a, args);
        assert!(!denied.status.success(), "{args:?}");
        assert!(
            err(&denied).contains("not found in this project"),
            "{}",
            err(&denied)
        );
    }
    assert!(!b.join(".yoke").exists() && !a.join(".yoke").exists());
    let empty = run(&home, &a, &["recall"]);
    assert!(out(&empty).starts_with("project: "), "{}", out(&empty));
    assert!(out(&empty).contains("No memories in this project"));
    let explained = run(&home, &b, &["recall", "--explain"]);
    assert!(
        out(&explained).contains("injection: \"inactive"),
        "{}",
        out(&explained)
    );

    // Sharing twice says so, and forgetting removes the shared copy.
    assert!(out(&run(&home, &b, &["share", &id])).contains("Shared knowledge takes effect"));
    assert!(out(&run(&home, &b, &["share", &id])).contains("already shared"));
    let knowledge = b.join(".yoke/knowledge.md");
    assert!(fs::read_to_string(&knowledge)
        .unwrap()
        .contains("migration reversible"));
    let forgotten = run(&home, &b, &["forget", &id]);
    assert!(forgotten.status.success(), "{}", err(&forgotten));
    assert!(out(&forgotten).contains("deleted: true"));
    assert!(out(&forgotten).contains("shared_copy: removed from"));
    assert!(!fs::read_to_string(&knowledge)
        .unwrap()
        .contains("migration reversible"));
    assert!(!run(&home, &b, &["forget", &id]).status.success());
}

#[test]
fn import_and_digest_failures_name_the_next_step() {
    let (temp, home, a, b) = fresh();
    run(&home, &b, &["remember", "Portable note"]);
    let archive = temp.path().join("memory.json");
    assert!(
        run(&home, &b, &["memory", "export", archive.to_str().unwrap()])
            .status
            .success()
    );
    let source = fs::canonicalize(&b).unwrap().display().to_string();
    let elsewhere = fs::canonicalize(&a).unwrap().display().to_string();
    let file = archive.to_str().unwrap();

    let unmapped = run(&home, &b, &["memory", "import", file, "--dry-run"]);
    assert!(
        err(&unmapped).contains(&format!("--map {source}={source}")),
        "{}",
        err(&unmapped)
    );

    // Importing into another state first, under a different path, then asking for the original.
    let other = temp.path().join("other-home");
    fs::create_dir(&other).unwrap();
    let moved = format!("{source}={elsewhere}");
    assert!(
        run(&other, &a, &["memory", "import", file, "--map", &moved])
            .status
            .success()
    );
    // A dry run refuses what the real import refuses, and says why.
    let in_place = format!("{source}={source}");
    let dry = run(
        &other,
        &a,
        &["memory", "import", file, "--map", &in_place, "--dry-run"],
    );
    assert!(!dry.status.success());
    assert!(
        out(&dry).contains(&format!("already stored under project {elsewhere}")),
        "{}",
        out(&dry)
    );

    let digest = run(&home, &b, &["memory", "digest", "some-session"]);
    assert!(err(&digest).contains("digest = true"), "{}", err(&digest));
}
