use super::*;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::time::{Duration, Instant};

const TARGET: &str = "aarch64-apple-darwin";
const OLD: &[u8] = b"#!/bin/sh\nprintf old";
const NEW: &[u8] = b"#!/bin/sh\nprintf new";
/// As long as `NEW`, so the fixture's asset sizes stay right, and it fails when run.
const UNRUNNABLE: &[u8] = b"#!/bin/sh\nexit 7    ";

fn fixture() -> (Release, Vec<u8>) {
    let name = format!("yoke-{TARGET}");
    let checksum = format!("{:x}  {name}\n", Sha256::digest(NEW)).into_bytes();
    let release = Release {
        tag_name: "v0.2.0".into(),
        draft: false,
        prerelease: false,
        assets: [
            (name.clone(), NEW.len()),
            ("SHA256SUMS".into(), checksum.len()),
        ]
        .into_iter()
        .map(|(name, size)| Asset {
            browser_download_url: format!(
                "https://github.com/{REPOSITORY}/releases/download/v0.2.0/{name}"
            ),
            name,
            size,
            state: "uploaded".into(),
        })
        .collect(),
    };
    (release, checksum)
}

fn old_executable(directory: &Path) -> std::path::PathBuf {
    let path = directory.join("yoke");
    fs::write(&path, OLD).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn replaces_local_executable_only_after_checksum_validation() {
    let temp = tempfile::tempdir().unwrap();
    let executable = old_executable(temp.path());
    let (release, checksum) = fixture();
    let mut fetch = |url: &str, _: usize| {
        Ok(if url.ends_with("/SHA256SUMS") {
            checksum.clone()
        } else {
            NEW.to_vec()
        })
    };
    install_release(&release, &executable, TARGET, &mut fetch).unwrap();
    assert_eq!(Command::new(&executable).output().unwrap().stdout, b"new");
    assert_eq!(
        fs::metadata(&executable).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn checksum_list_requires_one_exact_entry_and_rejects_malformed_records() {
    let name = format!("yoke-{TARGET}");
    let hash = format!("{:x}", Sha256::digest(NEW));
    let record = format!("{hash}  {name}\n");
    let list = format!("{hash}  other.tar.gz\n{record}{hash}  other.sha256\n");
    assert_eq!(parse_checksum(list.as_bytes(), &name).unwrap(), hash);
    for invalid in [
        String::new(),
        format!("{hash}  other\n"),
        record.repeat(2),
        format!("bad\n{record}"),
        format!("{hash} *{name}\n"),
        "x".repeat(CHECKSUM_LIMIT + 1),
    ] {
        assert!(parse_checksum(invalid.as_bytes(), &name).is_err());
    }
}

#[test]
fn download_and_checksum_failures_preserve_old_executable_and_leave_no_stage() {
    for failure in ["checksum", "download", "short", "filename", "unrunnable"] {
        let temp = tempfile::tempdir().unwrap();
        let executable = old_executable(temp.path());
        let (release, checksum) = fixture();
        let mut fetch = |url: &str, _: usize| {
            if url.ends_with("/SHA256SUMS") {
                let mut bytes = checksum.clone();
                if failure == "unrunnable" {
                    bytes =
                        format!("{:x}  yoke-{TARGET}\n", Sha256::digest(UNRUNNABLE)).into_bytes();
                }
                if failure == "checksum" {
                    bytes[0] = if bytes[0] == b'0' { b'1' } else { b'0' };
                }
                if failure == "filename" {
                    let last = bytes.len() - 2;
                    bytes[last] = b'/';
                }
                Ok(bytes)
            } else if failure == "download" {
                bail!("fixture network failure")
            } else if failure == "short" {
                Ok(vec![0])
            } else if failure == "unrunnable" {
                Ok(UNRUNNABLE.to_vec())
            } else {
                Ok(NEW.to_vec())
            }
        };
        assert!(
            install_release(&release, &executable, TARGET, &mut fetch).is_err(),
            "{failure}"
        );
        assert_eq!(Command::new(&executable).output().unwrap().stdout, b"old");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}

#[test]
fn rejects_untrusted_metadata_before_downloading_assets() {
    let temp = tempfile::tempdir().unwrap();
    let executable = old_executable(temp.path());
    for failure in [
        "tag",
        "url",
        "name",
        "size",
        "duplicate",
        "target",
        "missing-checksum",
    ] {
        let (mut release, _) = fixture();
        match failure {
            "tag" => release.tag_name = "../../escape".into(),
            "url" => release.assets[0].browser_download_url = "https://github.com/evil/yoke".into(),
            "name" => release.assets[0].name = "../yoke".into(),
            "size" => release.assets[0].size = BINARY_LIMIT + 1,
            "missing-checksum" => release.assets.retain(|asset| asset.name != "SHA256SUMS"),
            "duplicate" => {
                let (other, _) = fixture();
                release
                    .assets
                    .push(other.assets.into_iter().next().unwrap());
            }
            _ => (),
        }
        let mut fetch =
            |_: &str, _: usize| -> Result<Vec<u8>> { panic!("invalid metadata fetched assets") };
        assert!(install_release(
            &release,
            &executable,
            if failure == "target" {
                "../bad"
            } else {
                TARGET
            },
            &mut fetch
        )
        .is_err());
        assert_eq!(fs::read(&executable).unwrap(), OLD);
    }
}

#[test]
fn check_only_records_valid_version_and_keeps_previous_on_bad_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path().join("state.db")).unwrap();
    let valid = br#"{"tag_name":"v0.2.0","draft":false,"prerelease":false,"assets":[]}"#;
    let mut calls = 0;
    let latest = check_with(&store, &mut |url, limit| {
        assert_eq!(url, LATEST_URL);
        assert_eq!(limit, METADATA_LIMIT);
        calls += 1;
        Ok(valid.to_vec())
    })
    .unwrap();
    assert_eq!(latest.as_deref(), Some("v0.2.0"));
    assert_eq!(calls, 1);
    for invalid in [
        b"{}".as_slice(),
        br#"{"tag_name":"../v0.3.0","draft":false,"prerelease":false,"assets":[]}"#,
        br#"{"tag_name":"v0.3.0","draft":false,"prerelease":true,"assets":[]}"#,
    ] {
        assert!(check_with(&store, &mut |_, _| Ok(invalid.to_vec())).is_err());
        assert_eq!(
            store.get_kv(LATEST_VERSION_KEY).unwrap().as_deref(),
            Some("v0.2.0")
        );
    }
}

#[test]
fn bounded_process_rejects_excess_output_failure_and_deadline() {
    let mut output = Command::new("/bin/sh");
    output.args(["-c", "printf 12345"]);
    assert!(
        transport::collect("update download", &mut output, 4, Duration::from_secs(30))
            .unwrap_err()
            .to_string()
            .contains("size limit")
    );
    let mut failed = Command::new("/bin/sh");
    failed.args(["-c", "exit 9"]);
    let message = transport::collect("release lookup", &mut failed, 4, Duration::from_secs(30))
        .unwrap_err()
        .to_string();
    assert!(
        message.starts_with("release lookup failed (exit status: 9)"),
        "{message}"
    );
    assert!(!message.contains("download"), "{message}");
    let mut blocked = Command::new("/bin/sh");
    // The 40 ms deadline must cut the 30 s sleep short; the bound only tells a kill from a wait.
    blocked.args(["-c", "exec sleep 30"]);
    let start = Instant::now();
    assert!(transport::collect(
        "update download",
        &mut blocked,
        4,
        Duration::from_millis(40)
    )
    .is_err());
    assert!(start.elapsed() < Duration::from_secs(15));
}

#[test]
fn refuses_non_regular_destinations_without_touching_their_contents() {
    let temp = tempfile::tempdir().unwrap();
    let executable = old_executable(temp.path());
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&executable, &link).unwrap();
    assert!(replace(&link, NEW).is_err());
    assert_eq!(fs::read(&executable).unwrap(), OLD);
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    let directory = temp.path().join("directory");
    fs::create_dir(&directory).unwrap();
    assert!(replace(&directory, NEW).is_err());
    assert!(directory.is_dir());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 3);
}

#[test]
fn no_published_release_is_a_quiet_outcome_and_other_http_failures_stay_errors() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path().join("state.db")).unwrap();
    let mut missing = |_: &str, _: usize| -> Result<Vec<u8>> {
        Err(transport::HttpStatus::new("release lookup", 404).into())
    };
    assert_eq!(check_with(&store, &mut missing).unwrap(), None);
    assert_eq!(store.get_kv(LATEST_VERSION_KEY).unwrap(), None);
    let mut limited = |_: &str, _: usize| -> Result<Vec<u8>> {
        Err(transport::HttpStatus::new("release lookup", 429).into())
    };
    assert!(check_with(&store, &mut limited).is_err());
}

#[test]
fn curl_results_become_specific_messages_without_leaking_curl_exit_codes() {
    let split =
        |bytes: &[u8], limit| transport::split_status("release lookup", bytes.to_vec(), limit);
    assert_eq!(split(b"{}\n200", 8).unwrap(), b"{}");
    let missing = split(b"{\"message\":\"Not Found\"}\n404", 64).unwrap_err();
    assert_eq!(
        missing
            .downcast_ref::<transport::HttpStatus>()
            .unwrap()
            .code(),
        404
    );
    for (status, expected) in [
        (403, "rate limit"),
        (429, "rate limit"),
        (503, "try again later"),
    ] {
        let message = split(format!("x\n{status}").as_bytes(), 8)
            .unwrap_err()
            .to_string();
        assert!(message.starts_with("release lookup failed"), "{message}");
        assert!(message.contains(expected), "{message}");
        assert!(!message.contains("exit status"), "{message}");
    }
    assert!(split(b"junk", 8).is_err());
    assert!(split(b"123456789\n200", 8)
        .unwrap_err()
        .to_string()
        .contains("size limit"));
    for (code, expected) in [(6, "resolve"), (7, "connect"), (28, "timed out")] {
        let mut curl = Command::new("/bin/sh");
        curl.args(["-c", &format!("exit {code}")]);
        let message = transport::collect("release lookup", &mut curl, 4, Duration::from_secs(30))
            .unwrap_err()
            .to_string();
        assert!(message.contains(expected), "{message}");
    }
}

#[test]
fn curl_runs_from_a_fixed_path_and_never_negotiates_below_tls_1_2() {
    let temp = tempfile::tempdir().unwrap();
    let curl = temp.path().join("curl");
    fs::write(&curl, "").unwrap();
    let candidates = ["/nonexistent/curl", curl.to_str().unwrap()];

    assert_eq!(transport::curl_program(&candidates), curl);
    // With none of the fixed locations present, the PATH lookup is the last resort.
    assert_eq!(
        transport::curl_program(&["/nonexistent/curl"]),
        Path::new("curl")
    );
    assert!(transport::CURL_PATHS
        .iter()
        .all(|path| Path::new(path).is_absolute()));

    let command = transport::curl_command(&curl, "https://example.invalid/a", 10);
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(command.get_program(), curl.as_os_str());
    assert!(args.contains(&std::ffi::OsStr::new("--tlsv1.2")));
}

#[test]
fn yoke_curl_names_the_transport_only_in_dev_and_test_builds() {
    let fixed = ["/bin/sh"];
    let requested = || Some(std::ffi::OsString::from("/tmp/fake-curl"));

    // The shipped configuration (no debug assertions, no feature) ignores the override.
    assert!(!transport::honours_curl_override(false, false));
    assert_eq!(
        transport::select_curl(&fixed, requested(), false),
        Path::new("/bin/sh")
    );
    // Dev and test builds, and the feature build, honour it; an empty value never counts.
    assert!(transport::honours_curl_override(true, false));
    assert!(transport::honours_curl_override(false, true));
    assert_eq!(
        transport::select_curl(&fixed, requested(), true),
        Path::new("/tmp/fake-curl")
    );
    assert_eq!(
        transport::select_curl(&fixed, Some("".into()), true),
        Path::new("/bin/sh")
    );
}
