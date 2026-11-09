//! The Memory page: search by words, type and file; one memory in full (`memory?id=N`); what the
//! agent gets at session start; pending facts and digest state per session; how memory works.
//! Nothing here writes: "Digest now" goes through `insights::act` with a confirmation.
use super::super::{
    content,
    render::{self, encode, escape},
    BadRequest, Console,
};
use anyhow::Result;
use std::collections::BTreeMap;
use yoke_core::{analytics, config, memory, redact::redact, store::Store};

const LIST_LIMIT: usize = 100;
const DIGEST_SESSIONS: usize = 10;

fn time(ts: i64) -> String {
    format!("<time data-time=\"{ts}\">{ts}</time>")
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn bad_request(message: impl Into<String>) -> anyhow::Error {
    BadRequest(message.into()).into()
}

pub(super) fn page(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let project = c.project.to_string_lossy().into_owned();
    let text = |key: &str| {
        params
            .get(key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    };
    let mut out = render::heading(
        "memory",
        "Keep useful context close. Search what the project has learned, see what the agent gets at session start, and choose what stays.",
    );
    if let Some(id) = text("id") {
        let id = id.parse::<i64>().ok().filter(|id| *id > 0).ok_or_else(|| {
            bad_request(
                "A memory id is a positive whole number; open a memory from the Memory list",
            )
        })?;
        out.push_str(&detail(store, &project, id)?);
        out.push_str("</main>");
        return Ok(out);
    }
    let memory_type = text("type")
        .map(memory::parse_type)
        .transpose()
        .map_err(|error| bad_request(error.to_string()))?;
    let query = text("q").unwrap_or("");
    let file = text("file");
    // The same limits `recall_filtered` enforces, reported as a bad request instead of a failure.
    if query.len() > 4096 || file.is_some_and(|file| file.len() > 1024 || file.contains('\0')) {
        return Err(bad_request(
            "Search words are limited to 4096 bytes and a file path to 1024; shorten the search",
        ));
    }
    out.push_str(&filters(query, memory_type, file));
    out.push_str(&type_chips(store, &project, query, memory_type, file)?);
    let loaded = config::load_trusted(store, &c.project, &c.home);
    out.push_str(&injection_panel(store, &project, &loaded)?);
    out.push_str(&digest_panel(store, &project, &loaded)?);
    let rows = memory::recall_filtered(
        store,
        &project,
        &memory::RecallFilter {
            query,
            memory_type,
            file,
        },
        LIST_LIMIT,
    )?;
    if rows.is_empty() {
        out.push_str(&render::empty(
            "No matching memories",
            "Try other words, another type or a different file, or save a useful fact from your terminal.",
            "yoke remember --help",
        ));
    }
    for m in &rows {
        out.push_str(&memory_item(m));
    }
    out.push_str("<p class=\"muted\">Up to 100 matching memories. Sharing writes project knowledge; forgetting can be undone for 30 seconds.</p>");
    let (now, offset) = (
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64,
        super::super::usage::host_offset_minutes(),
    );
    out.push_str(&super::activity::type_timeline(
        store,
        Some(&project),
        now,
        offset,
    )?);
    out.push_str(&super::activity::project_activity(
        store, now, offset, &project,
    )?);
    out.push_str(&how_memory_works());
    out.push_str("</main>");
    Ok(out)
}

fn filters(query: &str, memory_type: Option<&str>, file: Option<&str>) -> String {
    let mut types = String::from("<option value=\"\">All types</option>");
    for name in memory::MEMORY_TYPES {
        types.push_str(&format!(
            "<option value=\"{name}\"{}>{name}</option>",
            if memory_type == Some(name) {
                " selected"
            } else {
                ""
            }
        ));
    }
    format!(
        r#"<form data-filter action="memory" class="filters memory-filters"><label class="grow">Search memories<input name="q" value="{}" placeholder="Words in a memory"></label><label>Type<select name="type">{types}</select></label><label class="grow">File<input name="file" value="{}" placeholder="src/lib.rs"></label><button>Search</button><a data-nav class="button quiet" href="memory">Reset</a></form>"#,
        escape(query),
        escape(file.unwrap_or(""))
    )
}

/// Links to each type that has memories, with all-time counts; the words and file stay applied.
fn type_chips(
    store: &Store,
    project: &str,
    query: &str,
    selected: Option<&str>,
    file: Option<&str>,
) -> Result<String> {
    let totals = analytics::memory_type_totals(store, Some(project))?;
    if totals.is_empty() {
        return Ok(String::new());
    }
    let keep = format!(
        "q={}&amp;file={}",
        encode(query),
        encode(file.unwrap_or(""))
    );
    let chip = |label: &str, count: i64, value: Option<&str>| {
        let query = match value {
            Some(value) => format!("type={value}&amp;{keep}"),
            None => keep.clone(),
        };
        format!(
            r#"<a data-nav class="memory-type" href="{}"{}>{} <span>{count}</span></a>"#,
            render::href("memory", &query),
            if value == selected {
                " aria-current=\"true\""
            } else {
                ""
            },
            escape(label)
        )
    };
    let mut out = String::from(r#"<nav class="memory-types" aria-label="Memory types">"#);
    out.push_str(&chip("All", totals.values().sum(), None));
    for name in memory::MEMORY_TYPES {
        if let Some(count) = totals.get(name) {
            out.push_str(&chip(name, *count, Some(name)));
        }
    }
    out.push_str("</nav>");
    Ok(out)
}

fn type_badge(memory_type: &str) -> String {
    format!(
        r#"<span class="badge memory-badge memory-{}">{}</span>"#,
        escape(memory_type),
        escape(memory_type)
    )
}

/// Pin, share and forget, handled by `console.js` like before.
fn memory_actions(m: &memory::Memory) -> String {
    format!(
        r#"<div class="actions action-zone"><button class="quiet" data-action="pin" data-id="{}" data-value="{}">{}</button><button class="quiet" data-action="share" data-id="{}" {}>{}</button><button class="quiet" data-confirm-forget="{}">Forget</button></div>"#,
        m.id,
        !m.pinned,
        if m.pinned { "Unpin" } else { "Pin" },
        m.id,
        if m.shared { "disabled" } else { "" },
        if m.shared { "Shared" } else { "Share" },
        m.id
    )
}

fn memory_item(m: &memory::Memory) -> String {
    format!(
        r#"<article class="panel memory" tabindex="0" data-row><div class="list-caption"><span>{} {}{}</span><span><a data-nav href="{}">Details</a> &middot; {}</span></div><div class="prose">{}</div><p class="muted">Source: {}</p>{}</article>"#,
        type_badge(&m.memory_type),
        if m.pinned { "Pinned" } else { "Memory" },
        if m.shared { " / Shared" } else { "" },
        render::href("memory", &format!("id={}", m.id)),
        time(m.ts),
        content::markdown(&m.text),
        escape(&m.source),
        memory_actions(m)
    )
}

fn detail(store: &Store, project: &str, id: i64) -> Result<String> {
    let back = r#"<p><a data-nav href="memory">Back to all memories</a></p>"#;
    let Some(found) = memory::detail(store, project, id)? else {
        return Ok(format!(
            r#"{back}<section class="empty"><h2>No memory #{id} in this project</h2><p>It may have been forgotten, or it belongs to another project.</p></section>"#
        ));
    };
    let m = &found.memory;
    let origin = if found.fact_count == 0 {
        "Saved directly, not from recorded facts".to_owned()
    } else {
        let sessions = found
            .sessions
            .iter()
            .map(|session| {
                format!(
                    r#"<a data-nav href="{}"><code>{}</code></a>"#,
                    render::href("decisions", &format!("session={}", encode(session))),
                    escape(session)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{} from {}: {sessions}",
            plural(found.fact_count, "recorded fact", "recorded facts"),
            if found.sessions.len() == 1 {
                "session"
            } else {
                "sessions"
            }
        )
    };
    let files = if found.files.is_empty() {
        "None named".to_owned()
    } else {
        let items: String = found
            .files
            .iter()
            .map(|file| {
                format!(
                    r#"<li><a data-nav href="{}"><code>{}</code></a></li>"#,
                    render::href("memory", &format!("file={}", encode(file))),
                    escape(file)
                )
            })
            .collect();
        format!("<ul class=\"memory-files\">{items}</ul>")
    };
    Ok(format!(
        r#"{back}<article class="panel memory memory-detail"><div class="list-caption"><h2>Memory #{}</h2>{}</div><div class="prose">{}</div><dl class="facts oversight"><dt>Saved</dt><dd>{}</dd><dt>Source</dt><dd>{}</dd><dt>Pinned</dt><dd>{}</dd><dt>Shared</dt><dd>{}</dd><dt>Written from</dt><dd>{origin}</dd><dt>Files it names</dt><dd>{files}</dd></dl>{}</article>"#,
        m.id,
        type_badge(&m.memory_type),
        content::markdown(&m.text),
        time(m.ts),
        escape(&m.source),
        if m.pinned {
            "Yes; injected at every session start in enforce mode"
        } else {
            "No"
        },
        if m.shared {
            "Yes; written to .yoke/knowledge.md, which applies after yoke trust"
        } else {
            "No"
        },
        memory_actions(m)
    ))
}

/// The exact block session start injects for this project, with why each part is there.
fn injection_panel(
    store: &Store,
    project: &str,
    loaded: &Result<config::LoadedConfig>,
) -> Result<String> {
    let mut out = String::from(
        r#"<section class="panel memory-inject"><h2>What the agent gets at session start</h2>"#,
    );
    let loaded = match loaded {
        Ok(loaded) => loaded,
        Err(error) => {
            out.push_str(&format!(
                "<p class=\"error\">The trusted configuration could not be loaded, so the selection cannot be shown: {}</p></section>",
                escape(&redact(&format!("{error:#}")))
            ));
            return Ok(out);
        }
    };
    let tokens = loaded.config.memory.inject_tokens;
    let items = memory::context_items(
        store,
        project,
        "",
        loaded.knowledge.as_deref().unwrap_or(""),
        tokens,
    )?;
    let used = items
        .iter()
        .map(|item| item.text.chars().count())
        .sum::<usize>()
        + items.len().saturating_sub(1);
    let limit = tokens.saturating_mul(4).min(1_000_000);
    out.push_str(&format!(
        "<p>{} {}, {used} of {limit} characters ({tokens} tokens, <code>memory.inject_tokens</code>).</p>",
        if loaded.config.mode == config::Mode::Enforce {
            "<span class=\"badge allow\">Active</span> Injected at every session start:"
        } else {
            "<span class=\"badge\">Inactive while mode is observe</span> This is what enforce mode would inject:"
        },
        plural(items.len(), "item", "items"),
    ));
    if items.is_empty() {
        out.push_str("<p class=\"muted\">Nothing is injected: there are no pinned notes, no trusted project knowledge and no recent notes.</p></section>");
        return Ok(out);
    }
    out.push_str(r#"<div class="table-scroll" tabindex="0" role="region" aria-label="Injected items"><table><caption>Injected items, in order</caption><thead><tr><th scope="col">Why</th><th scope="col">Memory</th><th scope="col">Source</th><th scope="col">Characters</th></tr></thead><tbody>"#);
    for item in &items {
        let why = match item.reason {
            "pinned" => "Pinned",
            "knowledge" => "Approved project knowledge",
            "recent" => "Recent",
            other => other,
        };
        let memory = match item.memory_id {
            Some(id) => format!(
                r#"<a data-nav href="{}">#{id}</a>"#,
                render::href("memory", &format!("id={id}"))
            ),
            None => "<code>.yoke/knowledge.md</code>".to_owned(),
        };
        out.push_str(&format!(
            "<tr><th scope=\"row\">{why}</th><td>{memory}</td><td>{}</td><td>{}{}</td></tr>",
            escape(item.source.as_deref().unwrap_or("trusted file")),
            item.text.chars().count(),
            if item.truncated { " (cut to fit)" } else { "" }
        ));
    }
    // Colons as character references read and copy the same, and keep link schemes such as
    // `javascript:` out of the page source, which the console never emits for stored text.
    out.push_str(&format!(
        "</tbody></table></div><details><summary>Exact text the agent receives</summary><pre><code>{}</code></pre></details><p class=\"muted\">The block labels itself untrusted data. Same as <code>yoke recall --explain</code> in a terminal.</p></section>",
        escape(&memory::fence(&items)).replace(':', "&#58;")
    ));
    Ok(out)
}

fn job_note(job: &yoke_core::jobs::Job) -> Option<String> {
    match job.state.as_str() {
        "queued" => Some("Digest queued".to_owned()),
        "running" => Some("Digest running".to_owned()),
        "failed" => Some(format!("Retry scheduled {}", time(job.run_after))),
        _ => None,
    }
}

/// Sessions with recorded facts, newest first, and a click-only "Digest now" for pending ones.
fn digest_panel(
    store: &Store,
    project: &str,
    loaded: &Result<config::LoadedConfig>,
) -> Result<String> {
    let states = memory::digest_states(store, project, DIGEST_SESSIONS)?;
    let mut out =
        String::from(r#"<section class="panel memory-digest"><h2>Pending facts and digests</h2>"#);
    if states.is_empty() {
        out.push_str("<p class=\"muted\">No facts recorded for this project yet. Hooks record them as your agent works.</p></section>");
        return Ok(out);
    }
    let blocker = match loaded {
        Ok(loaded) => super::digest_blocker(loaded).map(str::to_owned),
        Err(_) => Some(
            "Digest state is unknown because the trusted configuration could not be loaded."
                .to_owned(),
        ),
    };
    match (&blocker, loaded) {
        (Some(blocker), _) => out.push_str(&format!("<p>{}</p>", escape(blocker))),
        (None, Ok(loaded)) => out.push_str(&format!(
            "<p>Facts wait here until a digest turns them into memories. Digest now sends one session's pending facts to the <code>{}</code> CLI, and only after you confirm what will be sent.</p>",
            escape(&loaded.config.memory.digest_agent)
        )),
        (None, Err(_)) => {}
    }
    out.push_str(r#"<div class="table-scroll" tabindex="0" role="region" aria-label="Digest state by session"><table><caption>Up to 10 sessions with recorded facts, newest first</caption><thead><tr><th scope="col">Session</th><th scope="col">Pending</th><th scope="col">Digested</th><th scope="col">Last fact</th><th scope="col">Last digest</th><th scope="col">Digest</th></tr></thead><tbody>"#);
    for state in &states {
        let job = crate::memory::digest_job(store, project, &state.session_id)?;
        let action = if state.pending == 0 {
            "Nothing pending".to_owned()
        } else if blocker.is_some() {
            "Off".to_owned()
        } else if let Some(note) = job.as_ref().and_then(job_note) {
            note
        } else {
            let failed = job
                .as_ref()
                .and_then(|job| job.last_error.as_deref())
                .map(|error| {
                    format!(
                        "<p class=\"error\">The last digest failed: {}</p>",
                        escape(&redact(error))
                    )
                })
                .unwrap_or_default();
            format!(
                r#"{failed}<button type="button" class="quiet" data-memory-run="memory.digest" data-params="{}" data-confirm-label="Send for digest">Digest now</button>"#,
                escape(&serde_json::json!({ "session": state.session_id }).to_string())
            )
        };
        out.push_str(&format!(
            r#"<tr><th scope="row"><a data-nav href="{}"><code>{}</code></a></th><td>{} ({} characters)</td><td>{}</td><td>{}</td><td>{}</td><td class="action-zone">{action}</td></tr>"#,
            render::href("decisions", &format!("session={}", encode(&state.session_id))),
            escape(&state.session_id),
            plural(state.pending, "fact", "facts"),
            state.pending_chars,
            state.digested,
            time(state.last_fact_at),
            state.last_digest_at.map(time).unwrap_or_else(|| "Never".into())
        ));
    }
    out.push_str("</tbody></table></div></section>");
    Ok(out)
}

fn how_memory_works() -> String {
    r#"<details class="panel memory-how"><summary><h2>How memory works</h2></summary><ol><li>As your agent works, Yoke's hooks record facts in the local database: denied actions, file edits and failed Claude commands, with secrets masked.</li><li>A digest turns a session's pending facts into typed memories. It runs only when a person has trusted <code>digest = true</code> under <code>[memory]</code>, at the end of a session or at the next session start for sessions that ended without one. It sends the facts to the configured agent CLI. Digest now does the same for one session, after you confirm.</li><li>You can save notes yourself with <code>yoke remember</code>, typed with <code>--type</code>.</li><li>At session start in enforce mode, Yoke injects pinned notes, approved project knowledge, then recent notes, within <code>memory.inject_tokens</code>, in a block labelled untrusted data.</li><li>Only a person may pin. Sharing writes <code>.yoke/knowledge.md</code>, which applies after a person runs <code>yoke trust</code>. Everything else stays in <code>~/.yoke/state.db</code> on this computer.</li></ol></details>"#.to_owned()
}

#[cfg(test)]
mod tests {
    use super::super::tests::console;
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn memory_page_filters_by_type_and_file_and_opens_one_memory() {
        let (_temp, store, c) = console(false);
        let p = c.project.to_string_lossy().into_owned();
        let plain = memory::remember(&store, &p, "Plain <b>note</b>", false, 10).unwrap();
        let decided =
            memory::remember_typed(&store, &p, "Use WAL mode", "decision", false, 20).unwrap();
        // The list links each memory it shows to its detail page; the injection preview does not.
        let listed =
            |html: &str, id: i64| html.contains(&format!(r#"href="memory?id={id}">Details"#));
        let fact = memory::record_fact(
            &store,
            "sess-a",
            &p,
            "file_write",
            &format!(r#"Edit: {{"file_path":"{p}/src/jobs.rs","old_string":"a"}}"#),
            30,
        )
        .unwrap();
        // A digest whose agent answers with one typed memory written from that fact.
        let reply = serde_json::json!({"memories":[{"text":"Fixed the lease race","fact_ids":[fact],"type":"bugfix"}]}).to_string();
        let mut agent = std::process::Command::new("sh");
        agent.args([
            "-c",
            "cat >/dev/null; printf '%s' \"$1\"",
            "fixture",
            &reply,
        ]);
        memory::digest_with_command(
            &store,
            "sess-a",
            &p,
            "",
            40,
            &mut agent,
            std::time::Duration::from_secs(30),
            &mut || Ok(()),
        )
        .unwrap();
        let digested = memory::recall(&store, &p, "lease", 1).unwrap()[0].id;

        let all = page(&store, &c, &params(&[])).unwrap();
        assert!(all.contains(r#"href="memory?type=decision""#), "{all}");
        assert!(all.contains("All <span>3</span>"), "{all}");
        let typed = page(&store, &c, &params(&[("type", "Decision")])).unwrap();
        assert!(
            listed(&typed, decided.id) && !listed(&typed, plain.id) && !listed(&typed, digested)
        );
        assert!(
            listed(&all, plain.id) && !all.contains("<b>note"),
            "stored text is escaped"
        );
        let by_file = page(&store, &c, &params(&[("file", "src/jobs.rs")])).unwrap();
        assert!(listed(&by_file, digested) && !listed(&by_file, decided.id));

        let detail = page(&store, &c, &params(&[("id", &digested.to_string())])).unwrap();
        assert!(detail.contains("1 recorded fact from session"), "{detail}");
        assert!(
            detail.contains(r#"href="decisions?session=sess-a""#),
            "{detail}"
        );
        assert!(
            detail.contains(&format!(
                "href=\"memory?file={}\"",
                encode(&format!("{p}/src/jobs.rs"))
            )),
            "{detail}"
        );
        assert!(page(&store, &c, &params(&[("id", "999")]))
            .unwrap()
            .contains("No memory #999 in this project"));
        let error = page(&store, &c, &params(&[("type", "vibes")])).unwrap_err();
        assert!(error.downcast_ref::<BadRequest>().is_some(), "{error}");
    }

    #[test]
    fn memory_page_shows_the_injected_block_and_offers_digest_only_when_it_is_on() {
        let (_temp, store, c) = console(true);
        let p = c.project.to_string_lossy().into_owned();
        memory::remember(&store, &p, "Always run focused tests", true, 10).unwrap();
        memory::record_fact(&store, "s1", &p, "denial", "blocked", 20).unwrap();
        let html = page(&store, &c, &params(&[])).unwrap();
        assert!(html.contains("Inactive while mode is observe"), "{html}");
        assert!(
            html.contains("&lt;yoke-memory trust=&quot;untrusted-data&quot;&gt;"),
            "{html}"
        );
        assert!(html.contains("Always run focused tests"));
        assert!(html.contains(r#"data-memory-run="memory.digest" data-params="{&quot;session&quot;:&quot;s1&quot;}""#), "{html}");
        assert!(html.contains("1 fact (7 characters)") && html.contains("How memory works"));

        let (_temp, store, c) = console(false);
        let p = c.project.to_string_lossy().into_owned();
        memory::record_fact(&store, "s1", &p, "denial", "blocked", 20).unwrap();
        let off = page(&store, &c, &params(&[])).unwrap();
        assert!(
            off.contains("Digest is off") && !off.contains("data-memory-run"),
            "{off}"
        );
    }
}
