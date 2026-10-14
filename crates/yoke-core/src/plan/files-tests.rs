use std::os::unix::fs::{symlink, PermissionsExt};

use super::*;
use crate::plan::{self, storage};
use crate::store::Store;

const CHILD_PROJECT: &str = "YOKE_CHECK_LOCK_TEST_PROJECT";
const CHILD_TEST: &str = "plan::files::lock_tests::check_lock_child_cannot_resume_live_owner";

fn fixture() -> (tempfile::TempDir, Store, PlanRecord) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["commit", "--allow-empty", "-qm", "fixture seed"],
    ] {
        assert!(root.starts_with(std::env::temp_dir()));
        let output = Command::new("git")
            .current_dir(&root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .args([
                "-c",
                "user.name=Yoke Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgSign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let store = Store::open(temp.path().join("state.db")).unwrap();
    let plan = plan::create(&store, &root, ".yoke/plans", "example", "Example").unwrap();
    let approved = plan::prepare_approval(&store, &root, &plan.slug).unwrap();
    let approved = plan::approve(&store, &approved).unwrap();
    let checking =
        storage::transition(&store, &approved, "checking", "agent", None, None, None).unwrap();
    (temp, store, checking)
}

#[test]
fn check_lock_child_cannot_resume_live_owner() {
    let Some(root) = std::env::var_os(CHILD_PROJECT).map(PathBuf::from) else {
        return;
    };
    assert!(root.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    let store = Store::open(root.parent().unwrap().join("state.db")).unwrap();
    let error = plan::resume(&store, &root, "example").unwrap_err();
    assert!(error.to_string().contains("live process"), "{error:#}");
    assert_eq!(
        plan::get(&store, &root, "example").unwrap().state,
        "checking"
    );
}

#[test]
fn check_lock_survives_atomic_plan_replacement_across_processes() {
    let (_temp, store, plan) = fixture();
    let root = Path::new(&plan.project);
    let before = fingerprint(root).unwrap();
    let owner = claim_check(&plan).unwrap().unwrap();
    assert_eq!(
        fingerprint(root).unwrap(),
        before,
        "lock files must not enter check evidence"
    );

    let replacement = root.join("replacement.md");
    let content = fs::read_to_string(&plan.file).unwrap();
    fs::write(&replacement, content.replace("Example", "Edited")).unwrap();
    fs::rename(&replacement, &plan.file).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--quiet", "--test-threads=1"])
        .env(CHILD_PROJECT, root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child stdout: {}\nchild stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    drop(owner);
    let resumed = plan::resume(&store, root, "example").unwrap();
    assert_eq!(resumed.state, "building");
    assert_eq!(resumed.approved_header, plan.approved_header);
    assert!(plan::check(&store, root, "example", None)
        .unwrap_err()
        .to_string()
        .contains("header changed"));
}

#[test]
fn check_lock_refuses_unsafe_existing_lock_files() {
    let (_temp, _store, plan) = fixture();
    let lock = Path::new(&plan.project)
        .join(".git")
        .join(format!("yoke-check-{}.lock", hash(plan.file.as_bytes())));
    let target = Path::new(&plan.project).join("target");
    fs::write(&target, "unchanged").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&target, &lock).unwrap();
    assert!(claim_check(&plan).is_err());
    fs::remove_file(&lock).unwrap();
    fs::hard_link(&target, &lock).unwrap();
    assert!(claim_check(&plan).is_err());
    fs::remove_file(&lock).unwrap();
    fs::write(&lock, "").unwrap();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(claim_check(&plan).is_err());
    assert_eq!(fs::read_to_string(target).unwrap(), "unchanged");
}
