//! Faults in the state database that an ordinary run never meets.

use yoke_core::store::Store;

#[test]
fn a_claim_stamped_in_the_future_does_not_block_the_task_until_the_clock_catches_up() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("state.db")).unwrap();
    let hour = 3600;
    // The clock ran a year ahead once, then was corrected: the stored stamp is in the future.
    store
        .set_kv(
            "retention.last_prune",
            &(1_000_000_000 + 365 * 86_400).to_string(),
        )
        .unwrap();
    assert!(store
        .claim_interval("retention.last_prune", 1_000_000_000, hour)
        .unwrap());
    // The claim now holds the corrected time, so the interval applies again.
    assert!(!store
        .claim_interval("retention.last_prune", 1_000_000_000 + 10, hour)
        .unwrap());
}

#[test]
fn a_state_path_that_is_not_a_regular_file_is_refused_instead_of_waited_on() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("state.db");
    let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: the path is a fresh file name inside this test's temporary directory.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    for read_only in [false, true] {
        let (sender, receiver) = std::sync::mpsc::channel();
        let target = fifo.clone();
        std::thread::spawn(move || {
            let result = if read_only {
                Store::open_read_only(&target)
            } else {
                Store::open(&target)
            };
            let _ = sender.send(result.err().map(|error| format!("{error:#}")));
        });
        let error = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("opening a FIFO as the state database must not hang");
        assert!(
            error.is_some_and(|text| text.contains("not a regular file")),
            "read_only={read_only}"
        );
    }
}
