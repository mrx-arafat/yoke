use super::{
    content,
    render::{self, encode, escape},
    Console,
};
use anyhow::{ensure, Result};
use std::collections::BTreeMap;
use std::io::Read;
use yoke_core::{config, plan, store::Store};

#[path = "session_page.rs"]
mod session_page;

pub(super) fn sessions(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    // Session search and the session detail page (`sessions?id=`).
    session_page::page(store, c, params)
}

pub(super) fn plans(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let mut out = render::heading(
        "plans",
        "Review the contract, follow the checks, and decide when work can continue.",
    );
    if let Some(slug) = params.get("slug") {
        out.push_str(&super::plan_detail::page(store, c, slug)?);
    } else if let Some(worktree) = params.get("worktree") {
        out.push_str(&super::dashboard::plan_worktrees::detail(
            store, c, worktree,
        )?);
    } else {
        let plans = plan::list(store, &c.project)?;
        if plans.is_empty() {
            out.push_str(&render::empty(
                "Your next plan starts here",
                "Create a plan in your terminal, then return to review its contract and evidence.",
                "yoke plan new --help",
            ));
        }
        out.push_str(&super::dashboard::plan_stats::panel(store, c)?);
        out.push_str(&super::dashboard::plan_worktrees::panel(store, c)?);
        for p in plans {
            out.push_str(&format!(r#"<article class="panel"><div class="list-caption"><h2><a data-nav data-row href="{}">{}</a></h2><span class="badge {}">{}</span></div><p class="muted">Updated <time data-time="{}">{}</time> &middot; revision {}</p></article>"#,render::href("plans",&format!("slug={}",encode(&p.slug))),escape(&p.slug),escape(&p.state),escape(&p.state),p.updated_at,p.updated_at,p.revision));
        }
    }
    out.push_str("</main>");
    Ok(out)
}

pub(super) fn memories(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    // Type and file search, memory detail and the session-start preview.
    super::insights::memory_page(store, c, params)
}

pub(super) fn trust(store: &Store, c: &Console) -> Result<String> {
    let loaded = config::load_trusted(store, &c.project, &c.home)?;
    let mut out=render::heading("trust","See the configuration that actually runs. Trust changes remain an explicit terminal action.");
    out.push_str("<section class=\"panel\"><h2>Effective configuration</h2>");
    out.push_str(&format!(
        "<pre><code>{}</code></pre><h3>Origins</h3><dl class=\"facts settings-facts\">",
        content::highlight(&serde_json::to_string_pretty(&loaded.config)?)
    ));
    for (key, value) in loaded.origins {
        out.push_str(&format!(
            "<dt>{}</dt><dd>{}</dd>",
            escape(&key),
            escape(&value)
        ));
    }
    out.push_str("</dl></section>");
    for status in loaded.snapshots {
        // One wording for every snapshot state, shared with the Workflow settings table.
        let (_, state) = super::settings::snapshot_state(&status);
        out.push_str(&format!(
            "<section class=\"panel\"><h2>{}</h2><p>{} / {}</p>",
            escape(&status.file.display().to_string()),
            escape(&status.scope),
            state
        ));
        if let Some(error) = status.disk_error {
            out.push_str(&format!("<p class=\"error\">{}</p>", escape(&error)));
        }
        let root = if status.scope == "global" {
            &c.home
        } else {
            &loaded.project_root
        };
        let trusted = store.get_trust(
            &status.scope,
            &root.to_string_lossy(),
            &status.file.to_string_lossy(),
        )?;
        // Paths come only from trusted configuration discovery, never from request parameters.
        let disk = read_preview(&status.file);
        match disk {
            Ok(bytes) if bytes.len() <= 1024 * 1024 => {
                let disk = String::from_utf8(bytes)?;
                out.push_str(&content::diff(
                    trusted.as_ref().map(|v| v.content.as_str()).unwrap_or(""),
                    &disk,
                ));
            }
            Ok(_) => out.push_str("<p>Disk file exceeds the 1 MiB preview limit.</p>"),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                // A trusted snapshot keeps applying after its file is deleted, so show those bytes.
                match &trusted {
                    Some(row) => out.push_str(&format!(
                        "<p class=\"muted\">Not on disk. The trusted content below still applies.</p><pre><code>{}</code></pre>",
                        content::highlight(&row.content)
                    )),
                    None => out.push_str("<p class=\"muted\">Not on disk.</p>"),
                }
            }
            Err(error) => out.push_str(&format!("<p>{}</p>", escape(&error.to_string()))),
        }
        out.push_str("</section>");
    }
    out.push_str("<p class=\"muted\">Review and trust configuration from your terminal with <code>yoke trust --help</code>.</p></main>");
    Ok(out)
}

fn read_preview(path: &std::path::Path) -> Result<Vec<u8>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    anyhow::bail!("Safe configuration preview is unavailable on this platform.");
    let file = options.open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "Configuration preview requires a regular file; back up the policy entry and restore a regular UTF-8 file, then refresh Trust and review it with yoke trust in your own terminal."
    );
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}
