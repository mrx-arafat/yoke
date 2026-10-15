use super::*;

fn store() -> Result<(tempfile::TempDir, Store)> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.db"))?;
    Ok((directory, store))
}

#[test]
fn enqueue_is_idempotent_while_work_is_active() -> Result<()> {
    let (_directory, store) = store()?;
    let (first, created) = enqueue(&store, "memory_digest", "digest:a", "/p", "{}", 10)?;
    assert!(created);
    assert_eq!(
        enqueue(&store, "memory_digest", "digest:a", "/p", "{}", 11)?,
        (first.clone(), false)
    );
    let running = claim(&store, 10, 7, 60)?.unwrap();
    assert!(!enqueue(&store, "memory_digest", "digest:a", "/p", "{}", 12)?.1);
    assert_eq!(
        fail(&store, &running, 12, "token=abc123 broke", true)?,
        "failed"
    );
    let (pending, created) = enqueue(&store, "memory_digest", "digest:a", "/p", "{}", 13)?;
    assert!(!created);
    assert_eq!(
        pending.last_error.as_deref(),
        Some("token=[REDACTED] broke")
    );
    assert!(claim(&store, 13, 7, 60)?.is_none());
    let retried = claim(&store, 12 + backoff(1), 7, 60)?.unwrap();
    assert_eq!((retried.id, retried.attempts), (first.id, 2));
    assert_eq!(unfinished(&store, "digest:a")?.unwrap().state, "running");
    succeed(&store, &retried, 200, false)?;
    assert!(unfinished(&store, "digest:a")?.is_none());
    assert!(enqueue(&store, "memory_digest", "digest:a", "/p", "{}", 201)?.1);
    Ok(())
}

#[test]
fn expired_lease_returns_to_queue_and_fences_the_old_owner() -> Result<()> {
    let (_directory, store) = store()?;
    enqueue(&store, "memory_digest", "digest:a", "/p", "{}", 0)?;
    let lost = claim(&store, 0, 41, 100)?.unwrap();
    assert!(claim(&store, 0, 42, 100)?.is_none());
    assert_eq!(recover(&store, 100)?, 0);
    assert_eq!(recover(&store, 101)?, 1);
    let queued = &list(&store, false, 10)?[0];
    assert_eq!(
        (queued.state.as_str(), queued.owner_pid, queued.run_after),
        ("queued", None, 101 + backoff(1))
    );
    assert!(queued
        .last_error
        .as_deref()
        .unwrap()
        .contains("worker process 41"));
    assert!(!due(&store, 101)?);
    let owner = claim(&store, 101 + backoff(1), 42, 100)?.unwrap();
    assert!(heartbeat(&store, &lost, 102, 100).is_err());
    assert!(succeed(&store, &lost, 102, false).is_err());
    heartbeat(&store, &owner, 200, 100)?;
    assert_eq!(recover(&store, 250)?, 0);
    Ok(())
}

#[test]
fn exhausted_attempts_are_dead_until_a_human_retries() -> Result<()> {
    let (_directory, store) = store()?;
    let (job, _) = enqueue(&store, "memory_digest", "digest:a", "/p", "{}", 0)?;
    let mut now = 0;
    for attempt in 1..MAX_ATTEMPTS {
        let running = claim(&store, now, 1, 10)?.unwrap();
        assert_eq!(fail(&store, &running, now, "boom", true)?, "failed");
        now += backoff(attempt);
    }
    let last = claim(&store, now, 1, 10)?.unwrap();
    assert_eq!(last.attempts, MAX_ATTEMPTS);
    now += 11;
    assert_eq!(recover(&store, now)?, 1);
    assert_eq!(list(&store, false, 10)?[0].state, "dead");
    assert_eq!(unfinished(&store, "digest:a")?.unwrap().state, "dead");
    assert!(claim(&store, now + 100_000, 1, 10)?.is_none());
    let (other, _) = enqueue(&store, "memory_digest", "digest:a", "/p", "{}", now)?;
    let message = retry(&store, job.id, now).unwrap_err().to_string();
    assert!(message.contains(&format!(
        "job {} for the same work is already queued",
        other.id
    )));
    let unrelated = claim(&store, now, 1, 10)?.unwrap();
    assert_eq!(fail(&store, &unrelated, now, "disabled", false)?, "dead");
    let requeued = retry(&store, job.id, now)?;
    assert_eq!((requeued.state.as_str(), requeued.attempts), ("queued", 0));
    assert!(retry(&store, job.id, now).is_err());
    assert_eq!(prune(&store, now + 86_400 * 2, 1)?, 1);
    Ok(())
}

#[test]
fn success_and_its_follow_up_commit_together_or_not_at_all() -> Result<()> {
    let (_directory, store) = store()?;
    enqueue(
        &store,
        "memory_digest",
        "digest:a",
        "/p",
        "{\"session\":\"a\"}",
        0,
    )?;
    let running = claim(&store, 1, 7, 60)?.unwrap();
    // A worker killed between marking success and queuing the remainder must leave neither.
    store.connection().execute_batch(
        "CREATE TRIGGER crash BEFORE INSERT ON jobs BEGIN SELECT RAISE(ABORT,'crash window'); END",
    )?;
    assert!(succeed(&store, &running, 2, true).is_err());
    let rows = list(&store, true, 10)?;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].id, rows[0].state.as_str()),
        (running.id, "running")
    );
    store.connection().execute_batch("DROP TRIGGER crash")?;
    let follow_up = succeed(&store, &running, 3, true)?.unwrap();
    let rows = list(&store, true, 10)?;
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (rows[0].id, rows[0].state.as_str(), rows[0].attempts),
        (follow_up.id, "queued", 0)
    );
    assert_eq!(
        (rows[1].id, rows[1].state.as_str()),
        (running.id, "succeeded")
    );
    assert_eq!(
        (
            follow_up.kind.as_str(),
            follow_up.dedupe_key.as_str(),
            follow_up.project.as_str(),
            follow_up.payload.as_str(),
            follow_up.run_after
        ),
        ("memory_digest", "digest:a", "/p", "{\"session\":\"a\"}", 3)
    );
    assert_eq!(unfinished(&store, "digest:a")?.unwrap().id, follow_up.id);
    // Without a follow-up nothing is queued and the key is free again.
    let running = claim(&store, 3, 7, 60)?.unwrap();
    assert_eq!(running.id, follow_up.id);
    assert_eq!(succeed(&store, &running, 4, false)?, None);
    assert!(unfinished(&store, "digest:a")?.is_none());
    Ok(())
}
