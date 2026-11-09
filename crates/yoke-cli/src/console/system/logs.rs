//! Logs view: what Yoke can show locally, newest first. There is no daemon log; the sources are
//! hook errors recorded as decisions, background jobs that failed, `~/.yoke/error.log` (hook and
//! worker diagnostics) and this console's own recent errors.
use super::super::{
    actions::Outcome,
    render::{self, escape},
    Console,
};
use super::{int, size_text};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use yoke_core::{
    jobs,
    store::{DecisionFilter, Store},
};

/// Entries shown at most, newest first.
const LIMIT: usize = 200;
/// Bytes read from the end of each error.log generation.
const TAIL_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Error,
    Warning,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
        }
    }
}

struct Entry {
    ts: i64,
    level: Level,
    source: String,
    /// Empty for sources that belong to no project (error.log and the console).
    project: String,
    message: String,
}

fn log_paths(c: &Console) -> (PathBuf, PathBuf) {
    let state = c.home.join(".yoke");
    (state.join("error.log"), state.join("error.log.1"))
}

/// The last `TAIL_BYTES` of a regular file, or `None` when it does not exist. A link or special
/// file is refused rather than followed.
fn read_tail(path: &Path) -> Result<Option<String>> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file(),
        "{} is not a regular file",
        path.display()
    );
    let start = metadata.len().saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(metadata.len() - start).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    // A tail that starts mid-line drops the partial first line.
    Ok(Some(match (start > 0, text.split_once('\n')) {
        (true, Some((_, rest))) => rest.to_owned(),
        _ => text,
    }))
}

/// Every error.log line starts with a Unix timestamp; a line without one (a background worker's
/// raw stderr) takes the time of the line before it.
fn diagnostics(text: &str, last: &mut i64, out: &mut Vec<Entry>) {
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let (ts, message) = match line.split_once(' ') {
            Some((stamp, rest))
                if !stamp.is_empty() && stamp.bytes().all(|b| b.is_ascii_digit()) =>
            {
                (stamp.parse().unwrap_or(*last), rest)
            }
            _ => (*last, line),
        };
        *last = ts;
        out.push(Entry {
            ts,
            level: Level::Warning,
            source: "error.log".into(),
            project: String::new(),
            message: yoke_core::redact::redact(message),
        });
    }
}

/// Entries from every source, newest first, plus notes about sources that could not be read.
fn collect(store: &Store, c: &Console, project: Option<&str>) -> (Vec<Entry>, Vec<String>) {
    let mut entries = Vec::new();
    let mut notes = Vec::new();
    let filter = DecisionFilter {
        outcome: Some("error".into()),
        project: project.map(str::to_owned),
        limit: LIMIT,
        ..DecisionFilter::default()
    };
    match store.decisions(&filter) {
        Ok(rows) => entries.extend(rows.into_iter().map(|row| Entry {
            ts: row.ts,
            level: Level::Error,
            source: format!("Hook {} ({})", row.event, row.agent),
            project: row.project,
            message: match (&row.tool, &row.reason) {
                (Some(tool), Some(reason)) => format!("{tool}: {reason}"),
                (None, Some(reason)) => reason.clone(),
                (Some(tool), None) => format!("{tool}: no reason recorded"),
                (None, None) => "No reason recorded".into(),
            },
        })),
        Err(error) => notes.push(format!("Hook errors could not be read: {error:#}")),
    }
    match jobs::list(store, false, 1000) {
        Ok(rows) => entries.extend(
            rows.into_iter()
                .filter(|job| matches!(job.state.as_str(), "failed" | "dead"))
                .filter(|job| project.is_none_or(|project| job.project == project))
                .map(|job| Entry {
                    ts: job.updated_at,
                    level: if job.state == "dead" {
                        Level::Error
                    } else {
                        Level::Warning
                    },
                    source: format!("Job {} ({})", job.id, job.kind),
                    project: job.project,
                    message: format!(
                        "{}: {}",
                        if job.state == "dead" {
                            "dead, waiting for a human retry"
                        } else {
                            "failed, retrying automatically"
                        },
                        job.last_error.as_deref().unwrap_or("no error recorded")
                    ),
                }),
        ),
        Err(error) => notes.push(format!("Background jobs could not be read: {error:#}")),
    }
    if project.is_none() {
        let (current, rotated) = log_paths(c);
        let mut last = 0;
        for path in [rotated, current] {
            match read_tail(&path) {
                Ok(Some(text)) => diagnostics(&text, &mut last, &mut entries),
                Ok(None) => {}
                Err(error) => notes.push(format!("{error:#}")),
            }
        }
        entries.extend(
            super::console_errors()
                .into_iter()
                .map(|(ts, message)| Entry {
                    ts,
                    level: Level::Error,
                    source: "Console".into(),
                    project: String::new(),
                    message,
                }),
        );
    }
    // Stable, so error.log lines that share a second keep their file order (reversed).
    entries.reverse();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.ts));
    (entries, notes)
}

fn size(path: &Path) -> Option<u64> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
}

fn options(current: &str, choices: &[(&str, &str)]) -> String {
    choices
        .iter()
        .map(|(value, label)| {
            format!(
                r#"<option value="{}"{}>{}</option>"#,
                escape(value),
                if *value == current { " selected" } else { "" },
                escape(label)
            )
        })
        .collect()
}

pub(super) fn section(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let level = params.get("level").map_or("", String::as_str);
    let project = params.get("project").map_or("", String::as_str);
    let (all, mut notes) = collect(store, c, None);
    let mut projects: BTreeSet<String> = all
        .iter()
        .map(|entry| entry.project.clone())
        .filter(|project| !project.is_empty())
        .collect();
    projects.insert(c.project.display().to_string());
    if !project.is_empty() {
        projects.insert(project.to_owned());
    }
    let entries = if project.is_empty() {
        all
    } else {
        let (scoped, scoped_notes) = collect(store, c, Some(project));
        notes = scoped_notes;
        scoped
    };
    let shown: Vec<&Entry> = entries
        .iter()
        .filter(|entry| level.is_empty() || entry.level.name() == level)
        .take(LIMIT)
        .collect();
    let mut project_choices = vec![("", "All projects")];
    project_choices.extend(projects.iter().map(|p| (p.as_str(), p.as_str())));
    let mut out = format!(
        r#"<section class="panel sys-logs" aria-labelledby="sys-logs"><div class="list-caption"><h2 id="sys-logs">Logs</h2><span>{} shown &middot; newest first</span></div><form class="filters" data-filter action="system" method="get"><input type="hidden" name="view" value="logs"><label>Level<select name="level">{}</select></label><label>Project<select name="project">{}</select></label><button type="submit">Apply</button><button type="button" class="quiet" data-refresh>Refresh</button></form><p class="muted">Hook errors recorded as decisions, background jobs that failed, <code>~/.yoke/error.log</code> (hook and worker diagnostics, shown as warnings because its lines carry no level) and this console's own errors since it started. error.log and console entries belong to no project, so a project filter hides them.</p>"#,
        shown.len(),
        options(
            level,
            &[
                ("", "All levels"),
                ("error", "Errors"),
                ("warning", "Warnings")
            ]
        ),
        options(project, &project_choices),
    );
    for note in &notes {
        out.push_str(&format!(
            r#"<p class="error" role="status">{}</p>"#,
            escape(note)
        ));
    }
    if shown.is_empty() {
        out.push_str(&render::empty(
            "Nothing to show",
            "No hook errors, failed jobs or diagnostics match these filters.",
            "yoke log --outcome error",
        ));
    } else {
        out.push_str(r#"<div class="table-scroll" tabindex="0" role="region" aria-labelledby="sys-logs"><table class="sys-log-table"><thead><tr><th scope="col">Time</th><th scope="col">Level</th><th scope="col">Source</th><th scope="col">Message</th></tr></thead><tbody>"#);
        for entry in shown {
            let project = if entry.project.is_empty() {
                String::new()
            } else {
                format!(
                    r#"<small class="sys-project">{}</small>"#,
                    escape(&entry.project)
                )
            };
            let message = if entry.message.chars().count() > 180 {
                format!(
                    "<details><summary>{}</summary><p>{}</p></details>",
                    render::excerpt(&entry.message),
                    escape(&entry.message)
                )
            } else {
                escape(&entry.message)
            };
            out.push_str(&format!(
                r#"<tr><td><time data-time="{ts}">{}</time></td><td><span class="badge sys-level-{level}">{level}</span></td><td>{}{project}</td><td class="sys-message">{message}</td></tr>"#,
                escape(&yoke_core::timefmt::local_time_secs(entry.ts)),
                escape(&entry.source),
                ts = entry.ts,
                level = entry.level.name(),
            ));
        }
        out.push_str("</tbody></table></div>");
    }
    let (current, rotated) = log_paths(c);
    let (current_size, rotated_size) = (size(&current).unwrap_or(0), size(&rotated));
    let console = super::console_errors().len();
    if current_size > 0 || rotated_size.is_some() || console > 0 {
        let params = json!({
            "size": current_size,
            "rotated": rotated_size.map_or(-1, |size| size as i64),
        });
        out.push_str(&format!(
            r#"<div class="actions action-zone"><button type="button" class="danger-button" data-system-act="system.logs-clear" data-params="{}">Clear error.log</button><span class="muted">Hook errors and jobs are history and stay; see Retention.</span></div>"#,
            escape(&params.to_string())
        ));
    }
    out.push_str("</section>");
    Ok(out)
}

/// Empty error.log, delete error.log.1 and forget the console's in-memory errors, but only when
/// both files are still the size the human reviewed, so lines written since are never lost
/// unseen. Decisions and jobs are history and are never cleared here.
pub(super) fn clear(c: &Console, params: &Value, confirmed: bool) -> Result<Outcome> {
    let reviewed = u64::try_from(int(params, "size")?).context("Invalid size")?;
    let reviewed_rotated = u64::try_from(int(params, "rotated")?).ok();
    let (current, rotated) = log_paths(c);
    let unchanged = size(&current).unwrap_or(0) == reviewed && size(&rotated) == reviewed_rotated;
    ensure!(
        unchanged,
        "error.log changed since this view loaded; refresh to review the new lines, then clear again"
    );
    if !confirmed {
        let rotated_text = reviewed_rotated.map_or(String::new(), |size| {
            format!(", delete error.log.1 ({})", size_text(size))
        });
        return Ok(Outcome::Confirm(format!(
            "Empty {} ({}){rotated_text} and forget this console's {} in-memory errors. Hook errors recorded as decisions and background jobs are history and stay. This cannot be undone.",
            current.display(),
            size_text(reviewed),
            super::console_errors().len()
        )));
    }
    if reviewed > 0 {
        let file = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&current)
            .with_context(|| format!("open {}", current.display()))?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file() && metadata.len() == reviewed,
            "error.log changed since this view loaded; refresh to review the new lines, then clear again"
        );
        file.set_len(0)?;
    }
    if reviewed_rotated.is_some() {
        std::fs::remove_file(&rotated).with_context(|| format!("delete {}", rotated.display()))?;
    }
    let forgotten = super::clear_console_errors();
    Ok(Outcome::Done(json!({
        "message": format!(
            "Cleared error.log{} and {forgotten} console errors.",
            if reviewed_rotated.is_some() { " and error.log.1" } else { "" }
        )
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoke_core::store::DecisionRecord;

    fn decide(store: &Store, project: &Path, outcome: &str, reason: &str, ts: i64) {
        store
            .record_decision(
                &DecisionRecord {
                    policy_hash: None,
                    ts,
                    session_id: "s1".into(),
                    agent: "codex".into(),
                    project: project.display().to_string(),
                    event: "pre_tool_use".into(),
                    tool: Some("shell".into()),
                    mode: "enforce".into(),
                    outcome: outcome.into(),
                    reason: Some(reason.into()),
                    gates: serde_json::json!([]),
                    payload: "{}".into(),
                },
                4096,
            )
            .unwrap();
    }

    fn view(store: &Store, c: &Console, pairs: &[(&str, &str)]) -> String {
        let params = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        section(store, c, &params).unwrap()
    }

    #[test]
    fn logs_merge_sources_escape_redact_filter_and_clear_only_what_was_reviewed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let c = super::super::test_console(&root);
        let other = root.join("other");
        let store = Store::open(&c.database).unwrap();
        decide(
            &store,
            &c.project,
            "error",
            "gate <script>x</script> crashed",
            100,
        );
        decide(&store, &c.project, "deny", "ordinary denial", 101);
        decide(&store, &other, "error", "other-project-failure", 102);
        jobs::enqueue(&store, "retention", "retention", "", "{}", 50).unwrap();
        let job = jobs::claim(&store, 60, 7, 30).unwrap().unwrap();
        jobs::fail(&store, &job, 70, "disk is full", false).unwrap();
        let (log, rotated) = log_paths(&c);
        std::fs::write(&rotated, "90 older diagnostic\n").unwrap();
        std::fs::write(
            &log,
            "110 usage ingestion failed\nthread panicked token=sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789\n",
        )
        .unwrap();

        let html = view(&store, &c, &[]);
        assert!(
            html.contains("gate &lt;script&gt;x&lt;/script&gt; crashed")
                && !html.contains("<script>")
        );
        assert!(
            !html.contains("ordinary denial"),
            "only error outcomes are logs"
        );
        assert!(
            html.contains("other-project-failure"),
            "all projects by default"
        );
        assert!(html.contains("dead, waiting for a human retry: disk is full"));
        assert!(html.contains("older diagnostic") && html.contains("usage ingestion failed"));
        assert!(
            html.contains("thread panicked")
                && !html.contains("abcdefghijklmnopqrstuvwxyz0123456789")
        );
        // Newest first: the unstamped line takes 110 and sorts above the hook error at 102.
        let order = [
            "thread panicked",
            "usage ingestion",
            "other-project-failure",
            "gate &lt;",
            "older diagnostic",
            "disk is full",
        ];
        let at: Vec<usize> = order.iter().map(|text| html.find(text).unwrap()).collect();
        assert!(at.windows(2).all(|pair| pair[0] < pair[1]), "{at:?}");

        let project = c.project.display().to_string();
        let scoped = view(&store, &c, &[("project", &project)]);
        assert!(scoped.contains("gate &lt;script"));
        assert!(!scoped.contains("other-project-failure") && !scoped.contains("usage ingestion"));
        let warnings = view(&store, &c, &[("level", "warning")]);
        assert!(warnings.contains("usage ingestion") && !warnings.contains("gate &lt;script"));

        let size = std::fs::metadata(&log).unwrap().len() as i64;
        let reviewed = json!({"size": size, "rotated": 20});
        let Outcome::Confirm(message) = clear(&c, &reviewed, false).unwrap() else {
            panic!("clearing must ask first")
        };
        assert!(message.contains("delete error.log.1") && message.contains("cannot be undone"));
        assert_eq!(
            std::fs::metadata(&log).unwrap().len() as i64,
            size,
            "nothing changes before confirming"
        );
        // A line written after the human reviewed the log refuses the clear.
        std::fs::write(
            &log,
            format!("{}111 new line\n", std::fs::read_to_string(&log).unwrap()),
        )
        .unwrap();
        assert!(clear(&c, &reviewed, true).is_err());
        assert!(std::fs::read_to_string(&log)
            .unwrap()
            .contains("111 new line"));
        let size = std::fs::metadata(&log).unwrap().len() as i64;
        let reviewed = json!({"size": size, "rotated": 20});
        assert!(matches!(
            clear(&c, &reviewed, true).unwrap(),
            Outcome::Done(_)
        ));
        assert_eq!(std::fs::metadata(&log).unwrap().len(), 0);
        assert!(!rotated.exists());
        assert_eq!(
            store.decisions(&DecisionFilter::default()).unwrap().len(),
            3,
            "history stays"
        );
        // A link in place of error.log is never followed.
        let target = root.join("outside.txt");
        std::fs::write(&target, "keep").unwrap();
        std::fs::remove_file(&log).unwrap();
        std::os::unix::fs::symlink(&target, &log).unwrap();
        let html = view(&store, &c, &[]);
        assert!(html.contains(r#"role="status">read "#) && !html.contains("keep"));
        assert!(clear(&c, &json!({"size": 4, "rotated": -1}), true).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
    }
}
