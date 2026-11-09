//! Backups view: consistent copies of the state database in `~/.yoke/state-backups`, made with
//! SQLite's online backup API. Create, inspect, download and delete happen here; restoring is a
//! human-only terminal command, so the view shows its exact text instead of a button.
use super::super::{
    actions::Outcome,
    render::{self, encode, escape},
    BadRequest, Console,
};
use super::{now, shell_quote, size_text, Download};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use yoke_core::{backup, store::Store};

/// Bytes the live database occupies with its WAL, which a new backup roughly needs.
fn live_size(c: &Console) -> u64 {
    let mut wal = c.database.as_os_str().to_owned();
    wal.push("-wal");
    [c.database.as_path(), Path::new(&wal)]
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .sum()
}

fn find(c: &Console, name: &str) -> Result<backup::BackupFile> {
    if !backup::is_backup_name(name) {
        return Err(BadRequest("Choose a backup from the list.".into()).into());
    }
    backup::find(&backup::directory(&c.home), name)
}

fn details(c: &Console, name: &str) -> Result<String> {
    let file = find(c, name)?;
    let inspection = backup::inspect(&file.path)?;
    let tables: String = inspection
        .tables
        .iter()
        .map(|(table, rows)| {
            format!(
                r#"<tr><th scope="row">{}</th><td>{rows}</td></tr>"#,
                escape(table)
            )
        })
        .collect();
    let integrity = if inspection.integrity == "ok" {
        "ok".to_owned()
    } else {
        format!(
            r#"<span class="badge sys-level-error">failed</span> {}"#,
            escape(&inspection.integrity)
        )
    };
    let restore = match backup::restorable(&inspection) {
        Ok(()) => format!(
            r#"<p>Restoring replaces everything in the live database with this backup: decisions, sessions, memories, plans, jobs and the records of trusted policy. The console cannot restore. Stop your agents, then run this in your own terminal; it shows the backup again, asks you to type <code>restore</code> and writes a safety backup of the current state first:</p><pre><code>yoke backup restore {}</code></pre>"#,
            escape(&shell_quote(&file.path.display().to_string()))
        ),
        Err(error) => format!(
            r#"<p class="error" role="status">This backup cannot be restored: {}</p>"#,
            escape(&format!("{error:#}"))
        ),
    };
    Ok(format!(
        r#"<section class="panel" aria-labelledby="sys-backup-info"><div class="list-caption"><h2 id="sys-backup-info">{name}</h2><a data-nav href="system?view=backups">Close</a></div><dl class="facts oversight"><dt>Created</dt><dd><time data-time="{created}">{created_text}</time></dd><dt>Size</dt><dd>{size}</dd><dt>Schema</dt><dd>{schema} (this yoke opens 1 to {supported})</dd><dt>Integrity</dt><dd>{integrity}</dd><dt>SHA-256</dt><dd><code>{sha}</code></dd><dt>Path</dt><dd><code>{path}</code></dd></dl><div class="table-scroll" tabindex="0" role="region" aria-label="Rows in this backup"><table><thead><tr><th scope="col">Table</th><th scope="col">Rows</th></tr></thead><tbody>{tables}</tbody></table></div><h3>Restore</h3>{restore}<p><a class="button" href="system/download?backup={encoded}" download="{name}">Download</a></p></section>"#,
        name = escape(&file.name),
        created = file.created,
        created_text = escape(&yoke_core::timefmt::local_time_secs(file.created)),
        size = size_text(file.bytes),
        schema = inspection.schema,
        supported = inspection.supported,
        sha = escape(&inspection.sha256),
        path = escape(&file.path.display().to_string()),
        encoded = encode(&file.name),
    ))
}

pub(super) fn section(c: &Console, params: &BTreeMap<String, String>) -> Result<String> {
    let directory = backup::directory(&c.home);
    let mut out = match params.get("backup") {
        Some(name) => details(c, name)?,
        None => String::new(),
    };
    let backups = backup::list(&directory)?;
    let total: u64 = backups.iter().map(|file| file.bytes).sum();
    out.push_str(&format!(
        r#"<section class="panel" aria-labelledby="sys-backups"><div class="list-caption"><h2 id="sys-backups">Backups</h2><span>{} &middot; {}</span></div><p>A backup is a consistent copy of the whole state database, taken with SQLite's online backup API while agents keep working. Each is one private file in <code>{}</code>.</p><div class="actions action-zone"><button type="button" data-system-act="system.backup-create" data-params="{{}}">Create backup</button><span class="muted">Needs about {} of free disk space.</span></div>"#,
        match backups.len() {
            1 => "1 backup".to_owned(),
            n => format!("{n} backups"),
        },
        size_text(total),
        escape(&directory.display().to_string()),
        size_text(live_size(c)),
    ));
    if backups.is_empty() {
        out.push_str(&render::empty(
            "No backups yet",
            "Create one before a risky change; restoring one is a terminal command.",
            "yoke backup restore <file>",
        ));
    } else {
        out.push_str(r#"<div class="table-scroll" tabindex="0" role="region" aria-labelledby="sys-backups"><table><thead><tr><th scope="col">Backup</th><th scope="col">Created</th><th scope="col">Size</th><th scope="col">Actions</th></tr></thead><tbody>"#);
        for file in &backups {
            let encoded = encode(&file.name);
            out.push_str(&format!(
                r#"<tr><th scope="row"><a data-nav href="system?view=backups&amp;backup={encoded}">{name}</a></th><td><time data-time="{created}">{created_text}</time></td><td>{size}</td><td><div class="actions action-zone"><a href="system/download?backup={encoded}" download="{name}">Download</a><button type="button" class="quiet" data-system-act="system.backup-delete" data-params="{params}">Delete</button></div></td></tr>"#,
                name = escape(&file.name),
                created = file.created,
                created_text = escape(&yoke_core::timefmt::local_time_secs(file.created)),
                size = size_text(file.bytes),
                params = escape(&json!({"name": file.name}).to_string()),
            ));
        }
        out.push_str("</tbody></table></div>");
    }
    out.push_str("</section>");
    Ok(out)
}

pub(super) fn create(store: &Store, c: &Console) -> Result<Outcome> {
    let file = backup::create(store, &backup::directory(&c.home), now())?;
    Ok(Outcome::Done(json!({
        "message": format!("Backup {} created ({}).", file.name, size_text(file.bytes))
    })))
}

pub(super) fn delete(c: &Console, params: &Value, confirmed: bool) -> Result<Outcome> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .context("Missing backup name")?;
    let file = find(c, name)?;
    if !confirmed {
        return Ok(Outcome::Confirm(format!(
            "Delete backup {} ({}, created {}) from {}? It cannot be recovered.",
            file.name,
            size_text(file.bytes),
            yoke_core::timefmt::local_time_secs(file.created),
            backup::directory(&c.home).display()
        )));
    }
    std::fs::remove_file(&file.path).with_context(|| format!("delete {}", file.path.display()))?;
    Ok(Outcome::Done(
        json!({"message": format!("Backup {} deleted.", file.name)}),
    ))
}

/// The backup file itself, opened without following a link.
pub(super) fn download(c: &Console, name: &str) -> Result<Download> {
    let backup = find(c, name)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&backup.path)
        .with_context(|| format!("open {}", backup.path.display()))?;
    let metadata = file.metadata()?;
    anyhow::ensure!(metadata.is_file(), "backup is not a regular file");
    Ok(Download {
        file,
        length: metadata.len(),
        name: backup.name,
        kind: "application/vnd.sqlite3",
        _temporary: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backups_are_created_listed_inspected_downloaded_and_deleted_only_after_confirming() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let c = super::super::test_console(&root);
        let store = Store::open(&c.database).unwrap();
        let empty = section(&c, &BTreeMap::new()).unwrap();
        assert!(
            empty.contains("No backups yet")
                && empty.contains("data-system-act=\"system.backup-create\"")
        );

        let Outcome::Done(data) = create(&store, &c).unwrap() else {
            panic!("create needs no confirmation")
        };
        let name = backup::list(&backup::directory(&c.home)).unwrap()[0]
            .name
            .clone();
        assert!(data["message"].as_str().unwrap().contains(&name));
        let list = section(&c, &BTreeMap::new()).unwrap();
        assert!(list.contains(&format!(
            "href=\"system/download?backup={name}\" download=\"{name}\""
        )));
        let info = section(&c, &BTreeMap::from([("backup".to_owned(), name.clone())])).unwrap();
        assert!(info.contains("<dt>Integrity</dt><dd>ok</dd>"));
        assert!(
            info.contains("yoke backup restore &#39;")
                && !info.contains("data-system-act=\"system.restore")
        );
        let bad = section(
            &c,
            &BTreeMap::from([("backup".to_owned(), "../state.db".to_owned())]),
        );
        assert!(bad.unwrap_err().downcast_ref::<BadRequest>().is_some());

        let download = download(&c, &name).unwrap();
        assert_eq!(
            (download.name.as_str(), download.kind),
            (name.as_str(), "application/vnd.sqlite3")
        );
        assert_eq!(
            download.length,
            std::fs::metadata(backup::directory(&c.home).join(&name))
                .unwrap()
                .len()
        );
        assert!(super::super::download(
            &store,
            &c,
            &BTreeMap::from([("backup".to_owned(), "x/../../state.db".to_owned())])
        )
        .is_err());

        let params = json!({"name": name});
        let Outcome::Confirm(message) = delete(&c, &params, false).unwrap() else {
            panic!("delete must ask first")
        };
        assert!(message.contains("cannot be recovered"));
        assert_eq!(backup::list(&backup::directory(&c.home)).unwrap().len(), 1);
        delete(&c, &params, true).unwrap();
        assert!(backup::list(&backup::directory(&c.home))
            .unwrap()
            .is_empty());
    }
}
