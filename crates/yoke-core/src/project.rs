//! Project attribution and human-requested relocation of stored project records.

use crate::{process::Identity, store::Store};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Resolve a recorded hook session against the current tool process's OS ancestry.
pub fn agent_scope(
    store: &Store,
    agent: &str,
    session: Option<&str>,
    ancestors: &[Identity],
) -> Result<PathBuf> {
    let mut query = store.connection().prepare("SELECT session_id,root FROM session_anchors WHERE agent=?1 AND (?2 IS NULL OR session_id=?2) ORDER BY created_at DESC LIMIT 129")?;
    let candidates = query
        .query_map(params![agent, session], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(candidates.len() <= 128, "too many session anchors to identify this tool safely; start a new agent session in the project or run the memory command in your own terminal");
    let mut matches = Vec::new();
    for (session, root) in candidates {
        let mut statement = store.connection().prepare("SELECT payload FROM decisions WHERE session_id=?1 AND agent=?2 ORDER BY id DESC LIMIT 128")?;
        let mut rows = statement.query(params![session, agent])?;
        while let Some(row) = rows.next()? {
            let raw: String = row.get(0)?;
            let parent = serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|value| value.get("_yoke_parent").cloned())
                .and_then(|value| serde_json::from_value::<Identity>(value).ok());
            if parent
                .as_ref()
                .is_some_and(|parent| ancestors.contains(parent))
            {
                matches.push(PathBuf::from(root));
                break;
            }
        }
    }
    ensure!(matches.len() == 1, "memory command has no unique hook-recorded session anchor for this process; start a new agent session in the project, or run this command in your own terminal");
    matches.pop().context("missing matched session anchor")
}

#[derive(Debug, Default, serde::Serialize)]
pub struct MoveReport {
    pub tables: std::collections::BTreeMap<String, usize>,
    pub trust_moved: usize,
    pub trust_dropped: usize,
}

/// Move database attribution only; no files are renamed and no new trust is granted.
pub fn move_records(
    store: &Store,
    old: &Path,
    new: &Path,
    merge: bool,
    dry_run: bool,
) -> Result<MoveReport> {
    ensure!(old.is_absolute() && new.is_absolute() && !old.components().chain(new.components()).any(|part| part == std::path::Component::ParentDir), "project paths must be absolute without '..'; pass the recorded old path and the new directory");
    let new = new.canonicalize().context(
        "new project directory is unavailable; move the directory first, then move its records",
    )?;
    ensure!(
        new.is_dir() && old != new,
        "choose two different project directories"
    );
    let old = old.to_str().context("old project path must be UTF-8")?;
    let new = new.to_str().context("new project path must be UTF-8")?;
    ensure!(crate::redact::redact(old) == old && crate::redact::redact(new) == new, "project path is redacted in historical records, so attribution cannot be moved unambiguously; keep the original records and use a new project directory");
    if dry_run {
        let tx = store.connection().unchecked_transaction()?;
        relocate(&tx, old, new, merge, true)
    } else {
        store.transaction(|tx| relocate(tx,old,new,merge,false)).context("project records were not moved; resolve the reported conflict and retry with --dry-run")
    }
}

const PROJECT_TABLES: &[&str] = &[
    "decisions",
    "sessions",
    "facts",
    "memories",
    "plans",
    "usage_samples",
    "jobs",
    "notifications",
    "usage_session_totals",
];

fn exists(tx: &Transaction<'_>, table: &str) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
        [table],
        |row| row.get(0),
    )?)
}

fn relocate(
    tx: &Transaction<'_>,
    old: &str,
    new: &str,
    merge: bool,
    dry_run: bool,
) -> Result<MoveReport> {
    let old_hash = format!("sha256:{:x}", Sha256::digest(old.as_bytes()));
    let new_hash = format!("sha256:{:x}", Sha256::digest(new.as_bytes()));
    let mut report = MoveReport::default();
    let mut target = 0usize;
    for table in PROJECT_TABLES {
        if !exists(tx, table)? {
            continue;
        }
        // Table names come exclusively from the fixed schema allow-list above.
        let count = |raw: &str, hash: &str| -> Result<usize> {
            Ok(tx.query_row(
                &format!("SELECT count(*) FROM {table} WHERE project IN (?1,?2)"),
                params![raw, hash],
                |row| row.get(0),
            )?)
        };
        report
            .tables
            .insert((*table).into(), count(old, &old_hash)?);
        target += count(new, &new_hash)?;
    }
    let anchor_count = if exists(tx, "session_anchors")? {
        target += tx.query_row(
            "SELECT count(*) FROM session_anchors WHERE root=?1",
            [new],
            |row| row.get::<_, usize>(0),
        )?;
        tx.query_row(
            "SELECT count(*) FROM session_anchors WHERE root=?1",
            [old],
            |row| row.get(0),
        )?
    } else {
        0usize
    };
    report.tables.insert("session_anchors".into(), anchor_count);
    let target_trust: usize = tx.query_row(
        "SELECT count(*) FROM trust WHERE scope='project' AND root=?1",
        [new],
        |row| row.get(0),
    )?;
    target += target_trust;
    ensure!(target == 0 || merge, "target project already has {target} records; inspect both projects and explicitly add --merge if combining their histories is intended");
    let trust = tx
        .prepare(
            "SELECT file,sha256,content,trusted_at FROM trust WHERE scope='project' AND root=?1",
        )?
        .query_map([old], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(report.tables.values().sum::<usize>() > 0 || !trust.is_empty(), "no records found for the old project; pass the exact recorded project path and retry --dry-run");
    let mut transferred = Vec::new();
    for (file, hash, content, ts) in &trust {
        let relative = Path::new(file).strip_prefix(old).context("trust file is outside its recorded project; inspect the old trust records before retrying")?;
        let destination = Path::new(new).join(relative);
        if approved_bytes(&destination, Path::new(new), content)?
            && format!("{:x}", Sha256::digest(content.as_bytes())) == *hash
        {
            transferred.push((destination, hash, content, ts));
        }
    }
    // A policy is a batch: retaining only its matching includes can change its meaning.
    if transferred.len() != trust.len() {
        transferred.clear();
    }
    report.trust_moved = transferred.len();
    report.trust_dropped = trust.len() - transferred.len() + target_trust;
    report
        .tables
        .insert("trust".into(), trust.len() + target_trust);
    // Detect slug collisions before updating anything, with an actionable refusal.
    let conflict: Option<String> = tx.query_row("SELECT a.slug FROM plans a JOIN plans b ON a.slug=b.slug WHERE a.project=?1 AND b.project=?2 LIMIT 1",params![old,new],|row| row.get(0)).optional()?;
    ensure!(
        conflict.is_none(),
        "both projects contain plan {}; use a destination without that plan or keep the histories separate; no records changed",
        conflict.as_deref().unwrap_or("")
    );
    if dry_run {
        return Ok(report);
    }
    let plans = tx
        .prepare("SELECT id,file FROM plans WHERE project=?1")?
        .query_map([old], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, file) in plans {
        if let Ok(relative) = Path::new(&file).strip_prefix(old) {
            tx.execute(
                "UPDATE plans SET file=?1 WHERE id=?2",
                params![
                    Path::new(new)
                        .join(relative)
                        .to_str()
                        .context("new plan path must be UTF-8")?,
                    id
                ],
            )?;
        }
    }
    for table in PROJECT_TABLES {
        if !report.tables.contains_key(*table) {
            continue;
        }
        tx.execute(&format!("UPDATE {table} SET project=CASE WHEN project=?2 THEN ?4 ELSE ?3 END WHERE project IN (?1,?2)"),params![old,old_hash,new,new_hash])?;
    }
    if anchor_count > 0 {
        let anchors = tx
            .prepare("SELECT agent,session_id,anchor FROM session_anchors WHERE root=?1")?
            .query_map([old], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (agent, session, anchor) in anchors {
            let relative = Path::new(&anchor).strip_prefix(old).context("session start is outside its project; inspect the source session anchors before retrying")?;
            tx.execute(
                "UPDATE session_anchors SET root=?1,anchor=?2 WHERE agent=?3 AND session_id=?4",
                params![
                    new,
                    Path::new(new)
                        .join(relative)
                        .to_str()
                        .context("new anchor path must be UTF-8")?,
                    agent,
                    session
                ],
            )?;
        }
    }
    tx.execute(
        "DELETE FROM trust WHERE scope='project' AND root IN (?1,?2)",
        params![old, new],
    )?;
    for (file, hash, content, ts) in transferred {
        tx.execute("INSERT INTO trust(scope,root,file,sha256,content,trusted_at) VALUES('project',?1,?2,?3,?4,?5)",params![new,file.to_str().context("new trusted path must be UTF-8")?,hash,content,ts])?;
    }
    Ok(report)
}

fn approved_bytes(file: &Path, root: &Path, content: &str) -> Result<bool> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let Some(parent) = file.parent().and_then(|parent| parent.canonicalize().ok()) else {
        return Ok(false);
    };
    if !parent.starts_with(root) {
        return Ok(false);
    }
    let opened = match std::fs::File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(file)
    {
        Ok(file) => file,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) || error.raw_os_error() == Some(libc::ELOOP) =>
        {
            return Ok(false)
        }
        Err(error) => return Err(error.into()),
    };
    if !opened.metadata()?.is_file() || opened.metadata()?.len() != content.len() as u64 {
        return Ok(false);
    }
    let mut bytes = Vec::new();
    opened
        .take(content.len() as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes == content.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        memory,
        store::{DecisionRecord, TrustRow},
    };
    use sha2::Digest;

    #[test]
    fn agent_memory_scope_requires_the_recorded_parent_identity_and_session() {
        let store = Store::open(":memory:").unwrap();
        let parent = Identity {
            pid: 42,
            parent_pid: 1,
            name: "agent".into(),
            started: "boot:123".into(),
        };
        store
            .connection()
            .execute(
                "INSERT INTO session_anchors VALUES('codex','s','/anchored','/anchored',1)",
                [],
            )
            .unwrap();
        let mut row = DecisionRecord {
            policy_hash: None,
            ts: 1,
            session_id: "s".into(),
            agent: "codex".into(),
            project: "/anchored".into(),
            event: "SessionStart".into(),
            tool: None,
            mode: "enforce".into(),
            outcome: "allow".into(),
            reason: None,
            gates: serde_json::json!([]),
            payload: serde_json::json!({"_yoke_parent":parent}).to_string(),
        };
        store.record_decision(&row, 4096).unwrap();
        let mut truncated = row.clone();
        truncated.payload = "{\"tool_input\":\"truncated".into();
        store.record_decision(&truncated, 4096).unwrap();
        assert_eq!(
            agent_scope(&store, "codex", Some("s"), std::slice::from_ref(&parent)).unwrap(),
            Path::new("/anchored")
        );
        assert!(agent_scope(
            &store,
            "codex",
            Some("other"),
            std::slice::from_ref(&parent)
        )
        .is_err());
        let mut reused = parent.clone();
        reused.started = "boot:999".into();
        assert!(agent_scope(&store, "codex", Some("s"), &[reused]).is_err());
        row.session_id = "second".into();
        store
            .connection()
            .execute(
                "INSERT INTO session_anchors VALUES('codex','second','/other','/other',2)",
                [],
            )
            .unwrap();
        store.record_decision(&row, 4096).unwrap();
        assert!(agent_scope(&store, "codex", None, &[parent]).is_err());
    }

    #[test]
    fn project_move_previews_every_table_and_transfers_only_unchanged_trust() {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("old");
        let new = temp.path().join("new");
        std::fs::create_dir(&old).unwrap();
        std::fs::create_dir(&new).unwrap();
        let old = old.canonicalize().unwrap();
        let new = new.canonicalize().unwrap();
        let store = Store::open(":memory:").unwrap();
        memory::remember(&store, old.to_str().unwrap(), "moved note", false, 1).unwrap();
        let source = old.to_str().unwrap();
        memory::record_fact(&store, "move-session", source, "edit", "moved fact", 1).unwrap();
        store
            .record_decision(
                &DecisionRecord {
                    policy_hash: None,
                    ts: 1,
                    session_id: "move-session".into(),
                    agent: "codex".into(),
                    project: source.into(),
                    event: "SessionStart".into(),
                    tool: None,
                    mode: "enforce".into(),
                    outcome: "allow".into(),
                    reason: None,
                    gates: serde_json::json!([]),
                    payload: "{}".into(),
                },
                4096,
            )
            .unwrap();
        store
            .connection()
            .execute(
                "INSERT INTO session_anchors VALUES('codex','move-session',?1,?2,1)",
                params![old.join("sub").to_str().unwrap(), source],
            )
            .unwrap();
        store.connection().execute("INSERT INTO plans(project,slug,file,state,created_at,updated_at) VALUES(?1,'one',?2,'draft',1,1)",params![source,old.join(".yoke/plans/one.md").to_str().unwrap()]).unwrap();
        store.connection().execute("INSERT INTO jobs(kind,dedupe_key,project,payload,state,max_attempts,run_after,created_at,updated_at) VALUES('fixture','move',?1,'{}','queued',1,0,1,1)",[source]).unwrap();
        store.connection().execute("INSERT INTO notifications(key,kind,project,subject,title,detail,first_seen,last_seen) VALUES('move','config_untrusted',?1,'fixture','title','detail',1,1)",[format!("sha256:{:x}",sha2::Sha256::digest(source.as_bytes()))]).unwrap();
        store.connection().execute("INSERT INTO usage_samples(agent,session_id,project,record_key,model,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,ts) VALUES('codex','move-session',?1,'key','unknown',1,2,0,0,1)",[source]).unwrap();
        let content = "mode='enforce'\n";
        std::fs::write(new.join("yoke.toml"), content).unwrap();
        store
            .put_trust(&TrustRow {
                scope: "project".into(),
                root: old.to_str().unwrap().into(),
                file: old.join("yoke.toml").to_str().unwrap().into(),
                sha256: format!("{:x}", sha2::Sha256::digest(content.as_bytes())),
                content: content.into(),
                trusted_at: 1,
            })
            .unwrap();
        let knowledge = "Approved project knowledge";
        std::fs::create_dir_all(new.join(".yoke")).unwrap();
        std::fs::write(new.join(".yoke/knowledge.md"), knowledge).unwrap();
        store
            .put_trust(&TrustRow {
                scope: "project".into(),
                root: source.into(),
                file: old.join(".yoke/knowledge.md").to_str().unwrap().into(),
                sha256: format!("{:x}", sha2::Sha256::digest(knowledge.as_bytes())),
                content: knowledge.into(),
                trusted_at: 1,
            })
            .unwrap();
        let preview = move_records(&store, &old, &new, false, true).unwrap();
        assert_eq!(preview.tables.get("memories"), Some(&1));
        for table in PROJECT_TABLES
            .iter()
            .copied()
            .chain(["session_anchors", "trust"])
        {
            assert_eq!(
                preview.tables.get(table),
                Some(&if table == "trust" { 2 } else { 1 }),
                "{table}"
            );
        }
        assert_eq!(
            memory::recall(&store, old.to_str().unwrap(), "", 10)
                .unwrap()
                .len(),
            1
        );
        let report = move_records(&store, &old, &new, false, false).unwrap();
        assert_eq!(report.trust_moved, 2);
        for table in PROJECT_TABLES
            .iter()
            .copied()
            .filter(|table| *table != "notifications")
        {
            let count: usize = store
                .connection()
                .query_row(
                    &format!("SELECT count(*) FROM {table} WHERE project=?1"),
                    [new.to_str().unwrap()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "{table}");
        }
        let anchor: String = store
            .connection()
            .query_row(
                "SELECT anchor FROM session_anchors WHERE session_id='move-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(anchor, new.join("sub").to_str().unwrap());
        let plan_file: String = store
            .connection()
            .query_row("SELECT file FROM plans WHERE slug='one'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(plan_file, new.join(".yoke/plans/one.md").to_str().unwrap());
        let notice: String = store
            .connection()
            .query_row(
                "SELECT project FROM notifications WHERE key='move'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            notice,
            format!(
                "sha256:{:x}",
                sha2::Sha256::digest(new.to_str().unwrap().as_bytes())
            )
        );
        assert_eq!(
            memory::recall(&store, new.to_str().unwrap(), "", 10).unwrap()[0].text,
            "moved note"
        );
        assert_eq!(
            store
                .list_trust("project", new.to_str().unwrap())
                .unwrap()
                .len(),
            2
        );
        assert!(move_records(&store, &new, &old, false, true).is_ok());
        memory::remember(
            &store,
            old.to_str().unwrap(),
            "destination record",
            false,
            2,
        )
        .unwrap();
        assert!(move_records(&store, &new, &old, false, false).is_err());
        std::fs::write(old.join("yoke.toml"), "mode='observe'\n").unwrap();
        std::fs::create_dir_all(old.join(".yoke")).unwrap();
        std::fs::write(old.join(".yoke/knowledge.md"), knowledge).unwrap();
        let report = move_records(&store, &new, &old, true, false).unwrap();
        assert_eq!(report.trust_moved, 0);
        assert_eq!(report.trust_dropped, 2);
        assert!(store
            .list_trust("project", old.to_str().unwrap())
            .unwrap()
            .is_empty());
        assert_eq!(
            memory::recall(&store, old.to_str().unwrap(), "", 10)
                .unwrap()
                .len(),
            2
        );
        store.connection().execute("INSERT INTO plans(project,slug,file,state,created_at,updated_at) VALUES(?1,'one',?2,'draft',1,1)",params![new.to_str().unwrap(),new.join(".yoke/plans/one.md").to_str().unwrap()]).unwrap();
        assert!(move_records(&store, &old, &new, true, true).is_err());
        assert!(move_records(&store, &old, &new, true, false).is_err());
        assert_eq!(
            memory::recall(&store, old.to_str().unwrap(), "", 10)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn a_late_failure_rolls_back_every_project_table() {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().canonicalize().unwrap().join("old");
        let new = temp.path().canonicalize().unwrap().join("new");
        std::fs::create_dir(&old).unwrap();
        std::fs::create_dir(&new).unwrap();
        let store = Store::open(":memory:").unwrap();
        memory::remember(
            &store,
            old.to_str().unwrap(),
            "preserve on failure",
            false,
            1,
        )
        .unwrap();
        store
            .connection()
            .execute(
                "INSERT INTO sessions(id,agent,project,started_at) VALUES('s','codex',?1,1)",
                [old.to_str().unwrap()],
            )
            .unwrap();
        store.connection().execute_batch("CREATE TRIGGER refuse_move BEFORE UPDATE OF project ON memories BEGIN SELECT RAISE(ABORT,'fixture refusal'); END;").unwrap();
        assert!(move_records(&store, &old, &new, false, false).is_err());
        let project: String = store
            .connection()
            .query_row("SELECT project FROM sessions WHERE id='s'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(project, old.to_str().unwrap());
        assert_eq!(
            memory::recall(&store, old.to_str().unwrap(), "", 10).unwrap()[0].text,
            "preserve on failure"
        );
    }
}
