//! Read-only historical configuration. Never resolve a file path or use current trust here.
use super::{
    render::{self, escape},
    BadRequest, Console,
};
use anyhow::Result;
use std::collections::BTreeMap;
use yoke_core::store::Store;

#[derive(Debug)]
pub(super) struct MissingVersion;
impl std::fmt::Display for MissingVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The recorded policy version is unavailable. It cannot be reconstructed from the current configuration.")
    }
}
impl std::error::Error for MissingVersion {}

pub(super) fn page(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let hash = params
        .get("hash")
        .filter(|hash| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        .ok_or_else(|| BadRequest("Choose a policy version from a recorded decision.".into()))?;
    let version = store.policy_version(hash)?.ok_or(MissingVersion)?;
    let mut out = render::heading("policy", "The configuration captured when the hook loaded its policy. Current trusted files may differ.");
    out.push_str(&format!(r#"<p><a data-nav href="{}">Decisions</a> &middot; <a data-nav href="{}">Current trust &amp; config</a></p><section class="panel"><h2>Recorded version</h2><dl class="facts settings-facts"><dt>SHA-256</dt><dd><code>{}</code></dd><dt>Yoke version</dt><dd>{}</dd></dl><p class="muted">This archive records configuration, including built-in defaults. Gate executables, workspace files and plan state are not archived here.</p></section>"#, render::link(c, "decisions"), render::link(c, "trust"), escape(hash), escape(&version.yoke_version)));
    if version.sources.is_empty() {
        out.push_str("<section class=\"panel\"><h2>Built-in defaults only</h2><p>No trusted configuration files were loaded for this decision.</p></section>");
    }
    for source in version.sources {
        out.push_str(&format!(r#"<section class="panel"><h2>{}</h2><dl class="facts settings-facts"><dt>Scope</dt><dd>{}</dd><dt>Source SHA-256</dt><dd><code>{}</code></dd><dt>Trusted</dt><dd><time data-time="{}">{}</time></dd></dl><pre tabindex="0" role="region" aria-label="Trusted source text"><code>{}</code></pre></section>"#, escape(&source.file), escape(&source.scope), escape(&source.sha256), source.trusted_at, source.trusted_at, escape(&source.content)));
    }
    out.push_str(&format!("<details class=\"panel\"><summary>Effective configuration at the decision</summary><pre tabindex=\"0\" role=\"region\" aria-label=\"Effective configuration at the decision\"><code>{}</code></pre></details></main>", escape(&serde_json::to_string_pretty(&version.config)?)));
    Ok(out)
}
