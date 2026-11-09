//! Reading memories the way the console shows them: by type, by file, in full, and with the
//! digest state of the sessions that produced them. Nothing here writes.

use std::collections::BTreeSet;

use anyhow::{bail, Result};
use rusqlite::params;

use super::{memory_row, validate_project, Memory, Store, MEMORY_COLUMNS};
use crate::{redact::redact, touched};

/// The neutral type of a note nobody classified.
pub const NOTE_TYPE: &str = "note";

/// What a memory records: decision, discovery, bugfix, feature, refactor or change, or the neutral `note`.
pub const MEMORY_TYPES: [&str; 7] = [
    NOTE_TYPE,
    "decision",
    "discovery",
    "bugfix",
    "feature",
    "refactor",
    "change",
];

/// The canonical type for `text` (case and surrounding space ignored), or an error naming the choices.
pub fn parse_type(text: &str) -> Result<&'static str> {
    let wanted = text.trim().to_ascii_lowercase();
    match MEMORY_TYPES.iter().find(|known| **known == wanted) {
        Some(found) => Ok(found),
        None => bail!(
            "unknown memory type {text:?}; use one of {}",
            MEMORY_TYPES.join(", ")
        ),
    }
}

/// Words, a type and a file path; each one that is set narrows the result.
#[derive(Clone, Copy, Debug, Default)]
pub struct RecallFilter<'a> {
    /// Any of these words may match; see `recall`. Empty means every memory.
    pub query: &'a str,
    pub memory_type: Option<&'a str>,
    /// A path, absolute or relative to the project, found in the memory's text or in the tool
    /// inputs of the facts it was written from.
    pub file: Option<&'a str>,
}

/// `recall` with a type and a file filter, in the same order: pinned first when there are no
/// words, best match first when there are.
pub fn recall_filtered(
    store: &Store,
    project: &str,
    filter: &RecallFilter<'_>,
    limit: usize,
) -> Result<Vec<Memory>> {
    validate_project(project)?;
    let memory_type = filter.memory_type.map(parse_type).transpose()?;
    let (needle, alternate) = match filter.file.map(str::trim).filter(|file| !file.is_empty()) {
        Some(file) => {
            if file.len() > 1024 || file.contains('\0') {
                bail!("file filter must be a path of at most 1024 bytes");
            }
            let (a, b) = file_needles(project, file);
            (Some(a), Some(b))
        }
        None => (None, None),
    };
    let fts = super::fts_query(filter.query)?;
    let columns = MEMORY_COLUMNS
        .split(',')
        .map(|column| format!("m.{column}"))
        .collect::<Vec<_>>()
        .join(",");
    let (join, matching, order) = if fts.is_empty() {
        (
            "",
            "AND ?2=''",
            "ORDER BY m.pinned DESC,m.ts DESC,m.id DESC",
        )
    } else {
        (
            "JOIN memories_fts ON m.id=memories_fts.rowid",
            "AND memories_fts MATCH ?2",
            "ORDER BY bm25(memories_fts),m.pinned DESC,m.ts DESC,m.id DESC",
        )
    };
    let sql = format!(
        "SELECT {columns} FROM memories m {join}
         WHERE m.project=?1 {matching} AND (?3 IS NULL OR m.memory_type=?3)
         AND (?4 IS NULL OR instr(m.text,?4)>0 OR instr(m.text,?5)>0 OR EXISTS(
             SELECT 1 FROM memory_facts mf JOIN facts f ON f.id=mf.fact_id
             WHERE mf.memory_id=m.id AND (instr(f.detail,?4)>0 OR instr(f.detail,?5)>0)))
         {order} LIMIT ?6"
    );
    let mut statement = store.connection().prepare(&sql)?;
    let rows = statement.query_map(
        params![
            project,
            fts,
            memory_type,
            needle,
            alternate,
            limit.min(10_000) as i64
        ],
        memory_row,
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The two spellings a path can have in a record: as given and relative to or under the project.
fn file_needles(project: &str, file: &str) -> (String, String) {
    let prefix = format!("{}/", project.trim_end_matches('/'));
    match file.strip_prefix(&prefix) {
        Some(relative) => (file.to_owned(), relative.to_owned()),
        None if file.starts_with('/') => (file.to_owned(), file.to_owned()),
        None => {
            let relative = file.strip_prefix("./").unwrap_or(file);
            (relative.to_owned(), format!("{prefix}{relative}"))
        }
    }
}

/// One memory with where it came from and what it mentions.
#[derive(Clone, Debug)]
pub struct MemoryDetail {
    pub memory: Memory,
    /// Sessions whose facts this memory was written from; empty for a note typed by hand.
    pub sessions: Vec<String>,
    /// Facts it was written from.
    pub fact_count: usize,
    /// Paths in its text and in the tool inputs of those facts, first seen first.
    pub files: Vec<String>,
}

const DETAIL_FACT_LIMIT: i64 = 500;
const FILE_LIMIT: usize = 100;

/// The memory of this project with `id`; another project's id is `None`.
pub fn detail(store: &Store, project: &str, id: i64) -> Result<Option<MemoryDetail>> {
    let Some(memory) = super::in_scope(super::get(store, id)?, Some(project)) else {
        return Ok(None);
    };
    let connection = store.connection();
    let sessions = connection
        .prepare("SELECT DISTINCT f.session_id FROM memory_facts mf JOIN facts f ON f.id=mf.fact_id WHERE mf.memory_id=?1 ORDER BY f.session_id")?
        .query_map([id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let fact_count: usize = connection.query_row(
        "SELECT count(*) FROM memory_facts WHERE memory_id=?1",
        [id],
        |row| row.get(0),
    )?;
    let mut files = files_in_text(&memory.text);
    let details = connection
        .prepare("SELECT f.detail FROM memory_facts mf JOIN facts f ON f.id=mf.fact_id WHERE mf.memory_id=?1 ORDER BY f.id LIMIT ?2")?
        .query_map(params![id, DETAIL_FACT_LIMIT], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut from_facts = touched::Touched::default();
    for detail in &details {
        from_facts.merge(touched::files_in_payload(
            touched::fact_tool(detail),
            detail,
        ));
    }
    for path in from_facts.modified.into_iter().chain(from_facts.read) {
        if !files.contains(&path) {
            files.push(path);
        }
    }
    files.truncate(FILE_LIMIT);
    Ok(Some(MemoryDetail {
        memory,
        sessions,
        fact_count,
        files,
    }))
}

const KNOWN_EXTENSIONS: [&str; 22] = [
    "rs", "toml", "md", "json", "js", "ts", "tsx", "jsx", "py", "go", "sql", "css", "html", "sh",
    "yml", "yaml", "lock", "txt", "c", "h", "java", "rb",
];

/// Paths a note names: a word with a directory part and a file extension, or a bare file name
/// with a common source extension. Links, versions and abbreviations are not paths.
pub fn files_in_text(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    for word in text.split(|ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                '`' | '"' | '\'' | '(' | ')' | '[' | ']' | '<' | '>' | ',' | ';'
            )
    }) {
        let word = word.trim_end_matches(['.', ':', '!', '?']);
        // `src/a.rs:12` names the file; the line number is not part of the path.
        let word = match word.rsplit_once(':') {
            Some((path, line)) if line.bytes().all(|b| b.is_ascii_digit()) && !line.is_empty() => {
                path
            }
            _ => word,
        };
        if word.is_empty() || word.contains("://") || word.starts_with("//") || word.len() > 512 {
            continue;
        }
        let name = word.rsplit('/').next().unwrap_or(word);
        let Some((stem, extension)) = name.rsplit_once('.') else {
            continue;
        };
        let extension = extension.to_ascii_lowercase();
        let plausible = !stem.is_empty()
            && (1..=8).contains(&extension.len())
            && extension.bytes().all(|b| b.is_ascii_alphanumeric())
            && if word.contains('/') {
                extension.bytes().any(|b| b.is_ascii_alphabetic())
            } else {
                KNOWN_EXTENSIONS.contains(&extension.as_str())
            };
        if plausible && seen.insert(word.to_owned()) {
            files.push(redact(word));
        }
    }
    files
}

/// Pending and digested facts of one session, with the time of its last digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestState {
    pub session_id: String,
    pub pending: usize,
    /// Characters of detail in the pending facts, which is what a digest would send.
    pub pending_chars: usize,
    pub digested: usize,
    /// Time of the newest fact of the session.
    pub last_fact_at: i64,
    /// When a digest of this session last finished; `None` if none has.
    pub last_digest_at: Option<i64>,
}

/// Sessions of the project that have facts, newest activity first.
pub fn digest_states(store: &Store, project: &str, limit: usize) -> Result<Vec<DigestState>> {
    states(store, project, None, limit)
}

/// The digest state of one session of the project; `None` when it recorded no facts there.
pub fn digest_state(store: &Store, project: &str, session: &str) -> Result<Option<DigestState>> {
    Ok(states(store, project, Some(session), 1)?.pop())
}

fn states(
    store: &Store,
    project: &str,
    session: Option<&str>,
    limit: usize,
) -> Result<Vec<DigestState>> {
    validate_project(project)?;
    let mut statement = store.connection().prepare(
        "SELECT session_id,coalesce(sum(digested=0),0),coalesce(sum(CASE WHEN digested=0 THEN length(detail) END),0),coalesce(sum(digested=1),0),max(ts)
         FROM facts WHERE project=?1 AND (?3 IS NULL OR session_id=?3) GROUP BY session_id ORDER BY max(ts) DESC,session_id LIMIT ?2",
    )?;
    let rows = statement
        .query_map(params![project, limit.min(1000) as i64, session], |row| {
            Ok(DigestState {
                session_id: row.get(0)?,
                pending: row.get(1)?,
                pending_chars: row.get(2)?,
                digested: row.get(3)?,
                last_fact_at: row.get(4)?,
                last_digest_at: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|mut state| {
            let key = format!(
                "memory.digest.{}",
                super::digest_identity(&state.session_id, project)
            );
            state.last_digest_at = store
                .get_kv(&key)?
                .and_then(|value| value.parse::<i64>().ok());
            Ok(state)
        })
        .collect()
}

/// Memories of the project written from facts that this session recorded, newest first.
pub fn memories_of_session(
    store: &Store,
    project: &str,
    session: &str,
    limit: usize,
) -> Result<Vec<Memory>> {
    validate_project(project)?;
    let columns = MEMORY_COLUMNS
        .split(',')
        .map(|column| format!("m.{column}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut statement = store.connection().prepare(&format!(
        "SELECT {columns} FROM memories m WHERE m.project=?1 AND EXISTS(
             SELECT 1 FROM memory_facts mf JOIN facts f ON f.id=mf.fact_id
             WHERE mf.memory_id=m.id AND f.session_id=?2)
         ORDER BY m.ts DESC,m.id DESC LIMIT ?3"
    ))?;
    let rows = statement.query_map(
        params![project, redact(session), limit.min(10_000) as i64],
        memory_row,
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{record_fact, remember, remember_typed};

    fn store() -> Store {
        Store::open(":memory:").unwrap()
    }

    #[test]
    fn typed_notes_filter_by_type_and_a_plain_note_stays_neutral() {
        let store = store();
        let plain = remember(&store, "/p", "plain note", false, 1).unwrap();
        let decided = remember_typed(&store, "/p", "use WAL mode", "Decision", false, 2).unwrap();
        remember_typed(&store, "/q", "other project decision", "decision", false, 3).unwrap();
        assert_eq!(
            (plain.memory_type.as_str(), decided.memory_type.as_str()),
            ("note", "decision")
        );
        let only = |memory_type, query| {
            recall_filtered(
                &store,
                "/p",
                &RecallFilter {
                    query,
                    memory_type,
                    file: None,
                },
                10,
            )
            .unwrap()
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>()
        };
        assert_eq!(only(Some("decision"), ""), vec![decided.id]);
        assert_eq!(only(Some("decision"), "WAL"), vec![decided.id]);
        assert_eq!(only(Some("bugfix"), ""), Vec::<i64>::new());
        assert_eq!(only(None, ""), vec![decided.id, plain.id]);
        let error = remember_typed(&store, "/p", "x", "thought", false, 4).unwrap_err();
        assert!(
            error.to_string().contains("one of note, decision"),
            "{error}"
        );
        assert!(serde_json::to_string(&decided)
            .unwrap()
            .contains(r#""type":"decision""#));
    }

    #[test]
    fn file_search_and_detail_follow_the_note_text_and_the_facts_it_came_from() {
        let store = store();
        let named = remember(
            &store,
            "/p",
            "Fixed the race in crates/core/src/jobs.rs:42 today.",
            false,
            1,
        )
        .unwrap();
        let fact = record_fact(
            &store,
            "session-a",
            "/p",
            "file_write",
            r#"Edit: {"file_path":"/p/src/lib.rs","old_string":"a"}"#,
            2,
        )
        .unwrap();
        store.connection().execute_batch(&format!(
            "INSERT INTO memories(ts,project,source,text) VALUES (3,'/p','digest','Reworked the exports');
             INSERT INTO memory_facts(memory_id,fact_id) VALUES (2,{fact});
             INSERT INTO memories(ts,project,source,text) VALUES (4,'/q','digest','src/lib.rs elsewhere');"
        )).unwrap();
        let ids = |file| {
            recall_filtered(
                &store,
                "/p",
                &RecallFilter {
                    file: Some(file),
                    ..RecallFilter::default()
                },
                10,
            )
            .unwrap()
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>()
        };
        assert_eq!(ids("src/lib.rs"), vec![2]);
        assert_eq!(ids("/p/src/lib.rs"), vec![2]);
        assert_eq!(ids("jobs.rs"), vec![named.id]);
        assert_eq!(ids("missing.rs"), Vec::<i64>::new());
        let digest = detail(&store, "/p", 2).unwrap().unwrap();
        assert_eq!(
            (digest.sessions, digest.fact_count, digest.files),
            (
                vec!["session-a".to_owned()],
                1,
                vec!["/p/src/lib.rs".to_owned()]
            )
        );
        assert_eq!(
            detail(&store, "/p", named.id).unwrap().unwrap().files,
            vec!["crates/core/src/jobs.rs"]
        );
        assert!(
            detail(&store, "/p", 3).unwrap().is_none(),
            "another project's memory"
        );
        let of_session = |session| {
            memories_of_session(&store, "/p", session, 10)
                .unwrap()
                .iter()
                .map(|m| m.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            (of_session("session-a"), of_session("session-b")),
            (vec![2], vec![])
        );
    }

    #[test]
    fn a_digest_types_its_memories_and_an_archive_keeps_the_type() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path().join("state.db")).unwrap();
        let fact = record_fact(&store, "s", "/p", "denial", "blocked", 1).unwrap();
        let output = serde_json::json!({"memories":[
            {"text":"Root cause was the lock order","fact_ids":[fact],"type":"bugfix"},
            {"text":"Something vague","fact_ids":[fact],"type":"vibes"},
            {"text":"No type given","fact_ids":[fact]}]})
        .to_string();
        let mut command = std::process::Command::new("sh");
        command.args([
            "-c",
            "cat >/dev/null; printf '%s' \"$1\"",
            "fixture",
            &output,
        ]);
        crate::memory::digest_with_command(
            &store,
            "s",
            "/p",
            "",
            2,
            &mut command,
            std::time::Duration::from_secs(30),
            &mut || Ok(()),
        )
        .unwrap();
        let typed = |store: &Store, project: &str| {
            recall_filtered(store, project, &RecallFilter::default(), 10)
                .unwrap()
                .into_iter()
                .map(|m| (m.text, m.memory_type))
                .collect::<Vec<_>>()
        };
        let source = typed(&store, "/p");
        assert_eq!(source.iter().filter(|(_, t)| t == "bugfix").count(), 1);
        assert_eq!(source.iter().filter(|(_, t)| t == "note").count(), 2);
        let archive = directory.path().join("archive.json");
        crate::memory::export_archive(&store, &archive, Some("/p"), false).unwrap();
        let json = std::fs::read_to_string(&archive).unwrap();
        assert_eq!(
            json.matches("\"memory_type\"").count(),
            1,
            "plain notes are left out"
        );
        let other = Store::open(directory.path().join("other.db")).unwrap();
        let options = crate::memory::ImportOptions {
            dry_run: false,
            keep_pins: true,
            now: 10_000_000,
        };
        let mappings = std::collections::BTreeMap::from([("/p".to_owned(), "/p".to_owned())]);
        crate::memory::import_archive(&other, &archive, &mappings, &options).unwrap();
        let mut restored = typed(&other, "/p");
        let mut original = source;
        restored.sort();
        original.sort();
        assert_eq!(restored, original);
    }

    #[test]
    fn text_paths_skip_links_versions_and_abbreviations() {
        assert_eq!(
            files_in_text("See `src/a/b.rs`, Cargo.toml and (docs/x.md:7). https://x.dev/a.html v1.2.3 e.g. done."),
            vec!["src/a/b.rs", "Cargo.toml", "docs/x.md"]
        );
    }

    #[test]
    fn digest_state_counts_pending_facts_and_reads_the_last_digest_time() {
        let store = store();
        record_fact(&store, "s1", "/p", "file_write", "abcd", 10).unwrap();
        record_fact(&store, "s1", "/p", "denial", "xy", 11).unwrap();
        record_fact(&store, "s2", "/p", "denial", "z", 5).unwrap();
        store
            .connection()
            .execute("UPDATE facts SET digested=1 WHERE session_id='s2'", [])
            .unwrap();
        let key = format!(
            "memory.digest.{}",
            crate::memory::digest_identity("s2", "/p")
        );
        store
            .connection()
            .execute("INSERT INTO kv(key,value) VALUES (?1,'99')", [key])
            .unwrap();
        let states = digest_states(&store, "/p", 10).unwrap();
        assert_eq!(
            states
                .iter()
                .map(|s| (
                    s.session_id.as_str(),
                    s.pending,
                    s.pending_chars,
                    s.digested,
                    s.last_digest_at
                ))
                .collect::<Vec<_>>(),
            vec![("s1", 2, 6, 0, None), ("s2", 0, 0, 1, Some(99))]
        );
    }
}
