use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

use anyhow::{bail, Context, Result};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::{
    fact_row, memory_row, validate_project, validate_text, Fact, Memory, Store, MEMORY_COLUMNS,
};
use crate::redact::redact;

const ARCHIVE_LIMIT: usize = 16 * 1024 * 1024;
const RECORD_LIMIT: usize = 20_000;

struct ArchiveBuffer(Vec<u8>);

impl Write for ArchiveBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > ARCHIVE_LIMIT.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other(
                "serialized archive exceeds 16 MiB; a single oversized project is unsupported",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Archive {
    version: u32,
    projects: Vec<String>,
    memories: Vec<PortableMemory>,
    facts: Vec<PortableFact>,
    links: Vec<Link>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortableMemory {
    identity: String,
    ts: i64,
    project: String,
    source: String,
    text: String,
    pinned: bool,
    shared_reference: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortableFact {
    identity: String,
    ts: i64,
    session_id: String,
    project: String,
    kind: String,
    detail: String,
    digested: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
struct Link {
    memory: String,
    fact: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ExportReport {
    pub projects: Vec<String>,
    pub memories: usize,
    pub facts: usize,
    pub bytes: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportReport {
    pub projects: BTreeMap<String, String>,
    pub additions: usize,
    pub duplicates: usize,
    pub conflicts: Vec<String>,
    pub dry_run: bool,
}

pub fn export_archive(
    store: &Store,
    path: &Path,
    project: Option<&str>,
    overwrite: bool,
) -> Result<ExportReport> {
    let tx = store.connection().unchecked_transaction()?;
    let (count, size): (i64,i64) = tx.query_row(
        "SELECT count(*),coalesce(sum(bytes),0) FROM (
         SELECT length(CAST(text AS BLOB))+length(CAST(project AS BLOB))+256 AS bytes FROM memories WHERE (?1 IS NULL OR project=?1)
         UNION ALL SELECT length(CAST(detail AS BLOB))+length(CAST(project AS BLOB))+length(CAST(session_id AS BLOB))+length(CAST(kind AS BLOB))+256 FROM facts WHERE (?1 IS NULL OR project=?1))",
        [project],|row| Ok((row.get(0)?,row.get(1)?)))?;
    if count > RECORD_LIMIT as i64 || size > ARCHIVE_LIMIT as i64 {
        bail!("selected project scope exceeds supported archive limit (16 MiB or 20000 records); a single oversized project cannot currently be exported");
    }
    let mut memories = tx.prepare(&format!("SELECT {MEMORY_COLUMNS} FROM memories WHERE (?1 IS NULL OR project=?1) ORDER BY id LIMIT ?2"))?
        .query_map(params![project,RECORD_LIMIT as i64 + 1], memory_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut facts = tx.prepare("SELECT id,identity,ts,session_id,project,kind,detail,digested FROM facts WHERE (?1 IS NULL OR project=?1) ORDER BY id LIMIT ?2")?
        .query_map(params![project,RECORD_LIMIT as i64 + 1], fact_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    if memories.len() + facts.len() > RECORD_LIMIT {
        bail!("selected scope exceeds {RECORD_LIMIT} records; a single oversized project is unsupported");
    }
    let links = tx.prepare("SELECT m.identity,f.identity FROM memory_facts l JOIN memories m ON m.id=l.memory_id JOIN facts f ON f.id=l.fact_id WHERE (?1 IS NULL OR m.project=?1) ORDER BY m.identity,f.identity LIMIT ?2")?
        .query_map(params![project,RECORD_LIMIT as i64 + 1], |row| Ok(Link {memory:row.get(0)?,fact:row.get(1)?}))?.collect::<rusqlite::Result<Vec<_>>>()?;
    tx.commit()?;
    let projects = memories
        .iter()
        .map(|row| row.project.clone())
        .chain(facts.iter().map(|row| row.project.clone()))
        .chain(project.map(str::to_owned))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let archive = Archive {
        version: 1,
        projects: projects.clone(),
        memories: memories
            .drain(..)
            .map(|row| PortableMemory {
                identity: row.identity,
                ts: row.ts,
                project: row.project,
                source: row.source,
                text: row.text,
                pinned: row.pinned,
                shared_reference: row.shared,
            })
            .collect(),
        facts: facts
            .drain(..)
            .map(|row| PortableFact {
                identity: row.identity,
                ts: row.ts,
                session_id: row.session_id,
                project: row.project,
                kind: row.kind,
                detail: row.detail,
                digested: row.digested,
            })
            .collect(),
        links,
    };
    validate(&archive)?;
    let mut buffer = ArchiveBuffer(Vec::new());
    serde_json::to_writer_pretty(&mut buffer, &archive)?;
    let bytes = buffer.0;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    if overwrite {
        temporary.persist(path).map_err(|error| error.error)?;
    } else {
        temporary.persist_noclobber(path).map_err(|error| error.error).context("archive already exists or cannot be written; choose another path or explicit overwrite")?;
    }
    Ok(ExportReport {
        projects,
        memories: archive.memories.len(),
        facts: archive.facts.len(),
        bytes: bytes.len(),
    })
}

pub fn import_archive(
    store: &Store,
    path: &Path,
    mappings: &BTreeMap<String, String>,
    dry_run: bool,
) -> Result<ImportReport> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        bail!("archive must be a regular file, not a symlink or special file");
    }
    // O_NONBLOCK prevents a swapped FIFO from blocking; O_NOFOLLOW closes the symlink race.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)?;
    if !file.metadata()?.file_type().is_file() {
        bail!("archive must be a regular file");
    }
    if file.metadata()?.len() > ARCHIVE_LIMIT as u64 {
        bail!("archive exceeds 16 MiB");
    }
    let mut bytes = Vec::new();
    file.take(ARCHIVE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > ARCHIVE_LIMIT {
        bail!("archive exceeds 16 MiB");
    }
    let mut archive: Archive = serde_json::from_slice(&bytes).context("invalid memory archive")?;
    validate(&archive)?;
    if mappings.len() != archive.projects.len()
        || archive
            .projects
            .iter()
            .any(|project| !mappings.contains_key(project))
    {
        bail!(
            "provide exactly one explicit project mapping for each archive project: {:?}",
            archive.projects
        );
    }
    for destination in mappings.values() {
        validate_project(destination)?;
    }
    // Archives are untrusted plaintext. Apply the same persistence boundary as locally created notes.
    for memory in &mut archive.memories {
        memory.text = redact(&memory.text);
    }
    for fact in &mut archive.facts {
        fact.detail = redact(&fact.detail);
        fact.session_id = redact(&fact.session_id);
        fact.kind = redact(&fact.kind);
    }
    let tx = Transaction::new_unchecked(
        store.connection(),
        if dry_run {
            TransactionBehavior::Deferred
        } else {
            TransactionBehavior::Immediate
        },
    )?;
    let mut report = ImportReport {
        projects: mappings.clone(),
        additions: 0,
        duplicates: 0,
        conflicts: Vec::new(),
        dry_run,
    };
    let mut memory_ids = BTreeMap::new();
    let mut fact_ids = BTreeMap::new();
    for memory in &archive.memories {
        let existing = tx
            .query_row(
                &format!("SELECT {MEMORY_COLUMNS} FROM memories WHERE identity=?1"),
                [&memory.identity],
                memory_row,
            )
            .optional()?;
        match existing {
            Some(row) if same_memory(&row, memory, &mappings[&memory.project]) => {
                report.duplicates += 1;
                memory_ids.insert(memory.identity.clone(), row.id);
            }
            Some(_) => report.conflicts.push(format!(
                "memory {} differs from local record",
                memory.identity
            )),
            None => {
                report.additions += 1;
            }
        }
    }
    for fact in &archive.facts {
        let existing = tx.query_row("SELECT id,identity,ts,session_id,project,kind,detail,digested FROM facts WHERE identity=?1", [&fact.identity], fact_row).optional()?;
        match existing {
            Some(row) if same_fact(&row, fact, &mappings[&fact.project]) => {
                report.duplicates += 1;
                fact_ids.insert(fact.identity.clone(), row.id);
            }
            Some(_) => report
                .conflicts
                .push(format!("fact {} differs from local record", fact.identity)),
            None => {
                report.additions += 1;
            }
        }
    }
    // Existing identities must also agree on provenance; importing cannot silently alter it.
    for (identity, id) in &memory_ids {
        let expected = archive
            .links
            .iter()
            .filter(|link| &link.memory == identity)
            .map(|link| link.fact.clone())
            .collect::<BTreeSet<_>>();
        let actual = tx.prepare("SELECT f.identity FROM memory_facts l JOIN facts f ON f.id=l.fact_id WHERE l.memory_id=?1")?
            .query_map([id], |row| row.get(0))?.collect::<rusqlite::Result<BTreeSet<String>>>()?;
        if actual != expected {
            report.conflicts.push(format!(
                "memory {identity} has conflicting fact relationships"
            ));
        }
    }
    for (kind, ids) in [("memory", &memory_ids), ("fact", &fact_ids)] {
        for (identity, id) in ids {
            let mapped: Option<i64> = tx
                .query_row(
                    "SELECT local_id FROM imported_identities WHERE identity=?1 AND kind=?2",
                    params![identity, kind],
                    |row| row.get(0),
                )
                .optional()?;
            if mapped.is_some_and(|mapped| mapped != *id) {
                report.conflicts.push(format!(
                    "{kind} {identity} has a conflicting local identity mapping"
                ));
            }
        }
    }
    if dry_run {
        tx.commit()?;
        return Ok(report);
    }
    if !report.conflicts.is_empty() {
        bail!(
            "memory import conflicts; no records changed: {}",
            report.conflicts.join("; ")
        );
    }
    for memory in &archive.memories {
        if !memory_ids.contains_key(&memory.identity) {
            tx.execute("INSERT INTO memories(identity,ts,project,source,text,pinned,shared) VALUES (?1,?2,?3,?4,?5,?6,0)",
                params![memory.identity,memory.ts,mappings[&memory.project],memory.source,memory.text,memory.pinned])?;
            memory_ids.insert(memory.identity.clone(), tx.last_insert_rowid());
        }
        insert_identity(
            &tx,
            &memory.identity,
            "memory",
            memory_ids[&memory.identity],
        )?;
    }
    for fact in &archive.facts {
        if !fact_ids.contains_key(&fact.identity) {
            tx.execute("INSERT INTO facts(identity,ts,session_id,project,kind,detail,digested) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![fact.identity,fact.ts,fact.session_id,mappings[&fact.project],fact.kind,fact.detail,fact.digested])?;
            fact_ids.insert(fact.identity.clone(), tx.last_insert_rowid());
        }
        insert_identity(&tx, &fact.identity, "fact", fact_ids[&fact.identity])?;
    }
    for link in &archive.links {
        tx.execute(
            "INSERT INTO memory_facts(memory_id,fact_id) VALUES (?1,?2) ON CONFLICT DO NOTHING",
            params![memory_ids[&link.memory], fact_ids[&link.fact]],
        )?;
    }
    // INSERT triggers build the FTS index transactionally from imported local records.
    tx.commit()?;
    Ok(report)
}

fn insert_identity(tx: &Transaction<'_>, identity: &str, kind: &str, id: i64) -> Result<()> {
    let mapped: Option<i64> = tx
        .query_row(
            "SELECT local_id FROM imported_identities WHERE identity=?1 AND kind=?2",
            params![identity, kind],
            |row| row.get(0),
        )
        .optional()?;
    if mapped.is_some_and(|mapped| mapped != id) {
        bail!("conflicting imported identity mapping {identity}");
    }
    tx.execute("INSERT INTO imported_identities(identity,kind,local_id) VALUES (?1,?2,?3) ON CONFLICT DO NOTHING",params![identity,kind,id])?;
    Ok(())
}

fn same_memory(row: &Memory, incoming: &PortableMemory, project: &str) -> bool {
    row.ts == incoming.ts
        && row.project == project
        && row.source == incoming.source
        && row.text == incoming.text
        && row.pinned == incoming.pinned
}

fn same_fact(row: &Fact, incoming: &PortableFact, project: &str) -> bool {
    row.ts == incoming.ts
        && row.project == project
        && row.session_id == incoming.session_id
        && row.kind == incoming.kind
        && row.detail == incoming.detail
        && row.digested == incoming.digested
}

fn validate(archive: &Archive) -> Result<()> {
    if archive.version != 1 {
        bail!(
            "unsupported memory archive version {}; expected 1",
            archive.version
        );
    }
    if archive.memories.len() + archive.facts.len() > RECORD_LIMIT
        || archive.links.len() > RECORD_LIMIT
        || archive.projects.len() > 1000
    {
        bail!("archive record count exceeds limits");
    }
    let projects = archive.projects.iter().collect::<BTreeSet<_>>();
    if projects.len() != archive.projects.len() {
        bail!("duplicate archive project");
    }
    for project in &archive.projects {
        validate_project(project)?;
    }
    let mut memories = BTreeMap::new();
    let mut facts = BTreeMap::new();
    for memory in &archive.memories {
        validate_identity(&memory.identity)?;
        validate_text(&memory.text)?;
        if !projects.contains(&memory.project)
            || !matches!(memory.source.as_str(), "fact" | "remember" | "digest")
            || memories.insert(&memory.identity, memory).is_some()
        {
            bail!("invalid or duplicate memory {}", memory.identity);
        }
    }
    for fact in &archive.facts {
        validate_identity(&fact.identity)?;
        validate_text(&fact.detail)?;
        if !projects.contains(&fact.project)
            || fact.session_id.len() > 4096
            || fact.session_id.contains('\0')
            || fact.kind.is_empty()
            || fact.kind.len() > 128
            || fact.kind.contains('\0')
            || facts.insert(&fact.identity, fact).is_some()
        {
            bail!("invalid or duplicate fact {}", fact.identity);
        }
    }
    let mut links = BTreeSet::new();
    for link in &archive.links {
        let memory = memories
            .get(&link.memory)
            .context("link references absent memory")?;
        let fact = facts
            .get(&link.fact)
            .context("link references absent fact")?;
        if memory.project != fact.project || !fact.digested || !links.insert(link) {
            bail!("invalid, cross-project, pending, or duplicate fact relationship");
        }
    }
    Ok(())
}

fn validate_identity(identity: &str) -> Result<()> {
    if identity.len() != 32
        || !identity
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        bail!("invalid portable record identity");
    }
    Ok(())
}
