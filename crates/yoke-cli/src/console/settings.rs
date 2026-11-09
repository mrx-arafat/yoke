//! Workflow settings: what Yoke decides, where each value comes from, and what stays with the
//! native agent. Edits write a config file after a reviewed diff; only terminal trust applies them.
use super::{
    render::{self, escape},
    Console,
};
use anyhow::Result;
use yoke_core::{
    config::{self, LoadedConfig, Mode},
    store::Store,
    trust::SnapshotStatus,
};

#[path = "settings/edit.rs"]
mod edit;

pub(super) fn snapshot_state(status: &SnapshotStatus) -> (&'static str, &'static str) {
    let present = std::fs::symlink_metadata(&status.file).is_ok();
    match (
        status.trusted,
        status.stale,
        status.disk_error.is_some(),
        present,
    ) {
        (true, false, _, _) => ("trusted", "Trusted"),
        (true, true, true, _) => ("stale", "Trusted; missing or unreadable on disk (stale)"),
        (true, true, false, _) => ("stale", "Trusted; changed on disk (stale)"),
        (false, _, true, _) => ("untrusted", "Not trusted; unreadable on disk"),
        (false, _, false, true) => ("untrusted", "Not trusted (ignored)"),
        (false, _, false, false) => ("absent", "Not present"),
    }
}

fn origin(loaded: &LoadedConfig, key: &str) -> String {
    loaded
        .origins
        .get(key)
        .cloned()
        .unwrap_or_else(|| "default".into())
}

pub(super) fn page(store: &Store, c: &Console) -> Result<String> {
    let loaded = config::load_trusted(store, &c.project, &c.home)?;
    let cfg = &loaded.config;
    let mut out = render::heading(
        "settings",
        "What Yoke decides in your workflow, where each value comes from, and what stays with your coding agent.",
    );
    let (mode_class, mode_text) = match cfg.mode {
        Mode::Observe => (
            "would_deny",
            "Observe: gates run and would-deny decisions are recorded, but nothing is denied, rewritten or blocked.",
        ),
        Mode::Enforce => (
            "allow",
            "Enforce: gates can deny and rewrite actions, and active plans can block Stop.",
        ),
    };
    out.push_str(&format!(
        r#"<section class="panel" aria-labelledby="settings-activation"><h2 id="settings-activation">Activation</h2><p><span class="badge {mode_class}">{}</span> {}</p><dl class="facts settings-facts"><dt>Source</dt><dd><code>{}</code></dd><dt>Gates</dt><dd>{} trusted</dd></dl>"#,
        cfg.mode.as_str(),
        mode_text,
        escape(&origin(&loaded, "mode")),
        cfg.gates.len()
    ));
    if cfg.mode == Mode::Observe {
        out.push_str(&format!(
            r#"<p><a data-nav href="{}">How to turn on enforcement</a></p>"#,
            render::href("notifications", "q=observe")
        ));
    }
    out.push_str("</section>");

    out.push_str(r#"<section class="panel" aria-labelledby="settings-trust"><h2 id="settings-trust">Configuration trust</h2><p class="muted">Only trusted snapshots take effect. A stale file keeps applying its trusted bytes until a human trusts the new ones.</p><div class="table-scroll" tabindex="0" role="region" aria-labelledby="settings-trust"><table><thead><tr><th scope="col">File</th><th scope="col">Scope</th><th scope="col">State</th></tr></thead><tbody>"#);
    for status in &loaded.snapshots {
        let (class, text) = snapshot_state(status);
        out.push_str(&format!(
            r#"<tr><th scope="row"><code>{}</code></th><td>{}</td><td><span class="settings-trust-{class}">{text}</span></td></tr>"#,
            escape(&status.file.display().to_string()),
            escape(&status.scope)
        ));
    }
    out.push_str(&format!(
        r#"</tbody></table></div><p><a data-nav href="{}">Compare trusted and disk content</a></p></section>"#,
        render::link(c, "trust")
    ));

    out.push_str(&form(&loaded)?);

    out.push_str(&format!(
        r#"<section class="panel" aria-labelledby="settings-native"><h2 id="settings-native">Decided by your coding agent</h2><p>Yoke does not choose or switch the planning and building model. Neither Claude Code nor Codex documents a way for an outside tool to change the model of a running session, so these stay native and Yoke only shows what transcripts report.</p><dl class="facts settings-facts"><dt>Claude Code</dt><dd><code>/model</code>, <code>claude --model</code>, <code>ANTHROPIC_MODEL</code> or the <code>model</code> setting. The <code>opusplan</code> alias uses Opus in plan mode and Sonnet otherwise.</dd><dt>Codex</dt><dd><code>codex --model</code> (<code>-m</code>) or <code>model</code> in <code>~/.codex/config.toml</code>.</dd><dt>Reviewer and digest</dt><dd>Yoke launches these itself, so it passes the models above. Unset means the CLI default; Yoke never adds a fallback model, so an unavailable model fails with the CLI error.</dd></dl><p><a data-nav href="{}">See the models recorded in transcripts</a></p></section>"#,
        render::link(c, "usage")
    ));
    out.push_str(AGENT_HANDLED);
    out.push_str(r#"<p class="muted">Gates and environment allowances stay in the files. Saving here writes a file and never trusts it; you approve every change with <code>yoke trust</code> or <code>yoke trust --global</code> in your own terminal. Evaluation of model routing: <code>docs/decisions/0002-model-routing.md</code>.</p></main>"#);
    Ok(out)
}

/// Pilot Shell workflow toggles that have no Yoke setting, and who handles each instead.
const AGENT_HANDLED: &str = r#"<section class="panel" aria-labelledby="settings-elsewhere"><h2 id="settings-elsewhere">Handled outside these settings</h2><dl class="facts settings-facts"><dt>Model switching</dt><dd>Your agent picks the planning and building model; Yoke never switches it (see above).</dd><dt>Branch isolation</dt><dd>Yoke creates no branch or worktree for a plan. You or your agent make one; Plans shows the worktree a plan runs in.</dd><dt>Plan questions</dt><dd>Your agent decides when to ask. <code>yoke plan ask</code> records a question and the plan waits until you run <code>yoke plan resume</code>.</dd><dt>Plan approval</dt><dd>Always required and human-only: <code>yoke plan approve</code> in your terminal, or Approve on the Plans page.</dd><dt>Review mode and reviewers</dt><dd>Yoke runs one independent reviewer, the other agent from the one building; choose it with <code>review.reviewer</code> above.</dd><dt>Codex companion reviewers</dt><dd>No separate pass: set <code>review.reviewer</code> to <code>codex</code> (and <code>review.models.codex</code>) for plans Claude Code builds.</dd><dt>Console port</dt><dd>Chosen when you start the console: <code>yoke console --port &lt;n&gt;</code>, otherwise a free port on 127.0.0.1.</dd></dl></section>"#;

fn input(field: &edit::Field, value: Option<&str>) -> String {
    let id = format!("setting-{}", field.key.replace(['.', '_'], "-"));
    let name = escape(field.key);
    let select = |options: &[&str]| {
        let options: String = options
            .iter()
            .map(|option| {
                let selected = if Some(*option) == value {
                    " selected"
                } else {
                    ""
                };
                format!(
                    r#"<option value="{0}"{selected}>{0}</option>"#,
                    escape(option)
                )
            })
            .collect();
        format!(r#"<select id="{id}" name="{name}" data-setting>{options}</select>"#)
    };
    let value = escape(value.unwrap_or(""));
    match field.kind {
        edit::Kind::Bool => select(&["true", "false"]),
        edit::Kind::Choice(options) => select(options),
        edit::Kind::Count => format!(
            r#"<input id="{id}" name="{name}" data-setting type="number" min="0" step="1" inputmode="numeric" value="{value}">"#
        ),
        edit::Kind::Text => {
            format!(r#"<input id="{id}" name="{name}" data-setting type="text" value="{value}">"#)
        }
        edit::Kind::Model => format!(
            r#"<input id="{id}" name="{name}" data-setting type="text" placeholder="CLI default" spellcheck="false" value="{value}">"#
        ),
    }
}

/// The editable settings: effective values, their source, and what trusting the files on
/// disk would change. Saving goes through `settings.save` and a reviewed diff.
fn form(loaded: &LoadedConfig) -> Result<String> {
    let (project, global) = edit::files(loaded)?;
    let pending = edit::after_trust(loaded, &project, &global);
    let mut out = format!(
        r#"<section class="panel" aria-labelledby="settings-owned"><h2 id="settings-owned">Decided by Yoke</h2><p>Change values, choose the file, then review the exact edit before anything is written. A saved file applies only after you run <code>yoke trust</code> (<code>yoke trust --global</code> for the global file) in your own terminal.</p><form class="settings-form" data-settings-form data-base-project="{}" data-base-global="{}"><div class="table-scroll" tabindex="0" role="region" aria-labelledby="settings-owned"><table><thead><tr><th scope="col">Setting</th><th scope="col">Value</th><th scope="col">Source</th><th scope="col">What it controls</th></tr></thead><tbody>"#,
        escape(&edit::page_fingerprint(&project)),
        escape(&edit::page_fingerprint(&global))
    );
    for field in &edit::FIELDS {
        let value = edit::effective(&loaded.config, field.key);
        let waiting = edit::effective(&pending, field.key);
        let note = if waiting != value {
            format!(
                r#"<p class="settings-pending">Waiting for <code>yoke trust</code>: <code>{}</code></p>"#,
                escape(waiting.as_deref().unwrap_or("CLI default"))
            )
        } else {
            String::new()
        };
        out.push_str(&format!(
            r#"<tr><th scope="row"><label for="setting-{}"><code>{}</code></label></th><td>{}{note}</td><td><code>{}</code></td><td>{}</td></tr>"#,
            field.key.replace(['.', '_'], "-"),
            escape(field.key),
            input(field, value.as_deref()),
            escape(&origin(loaded, field.key)),
            escape(field.help)
        ));
    }
    out.push_str(&format!(
        r#"</tbody></table></div><fieldset class="settings-target"><legend>Save to</legend><label><input type="radio" name="target" value="project" checked> Project file <code>{}</code></label><label><input type="radio" name="target" value="global"> Global file <code>{}</code></label></fieldset><div class="actions"><button type="submit">Review the change</button><button type="reset" class="quiet">Discard edits</button></div></form></section>"#,
        escape(&project.display().to_string()),
        escape(&global.display().to_string())
    ));
    Ok(out)
}

/// Delegated `settings.*` console actions.
/// Edits may write a config file but never trust it; the human runs `yoke trust`.
pub(super) fn act(
    store: &Store,
    c: &Console,
    name: &str,
    params: &serde_json::Value,
    confirmed: bool,
) -> Result<super::actions::Outcome> {
    match name {
        "settings.save" => edit::save(store, c, params, confirmed),
        _ => anyhow::bail!("Unknown action"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_show_editable_values_origins_pending_edits_and_stale_trust() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (home, project) = (root.join("home"), root.join("project"));
        std::fs::create_dir_all(home.join(".yoke")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let store = Store::open(home.join(".yoke/state.db")).unwrap();
        let file = project.join("yoke.toml");
        std::fs::write(&file, "mode='enforce'\n[review]\nreviewer='codex'\n").unwrap();
        yoke_core::trust::prepare_project(&store, &project)
            .unwrap()
            .commit(&store)
            .unwrap();
        std::fs::write(&file, "mode='observe'\n").unwrap();
        let c = Console {
            home,
            project,
            database: root.join("unused.db"),
            token: "token".into(),
            csrf: "csrf".into(),
            host: "127.0.0.1:1".into(),
        };
        let html = page(&store, &c).unwrap();
        let source = escape(&file.display().to_string());
        // The trusted value fills the form; the edited disk file shows what trusting it changes.
        assert!(html.contains(&format!(
            "<option value=\"codex\" selected>codex</option></select><p class=\"settings-pending\">Waiting for <code>yoke trust</code>: <code>auto</code></p></td><td><code>{source}</code></td>"
        )), "{html}");
        assert!(html.contains(
            "name=\"stop.max_blocks\" data-setting type=\"number\" min=\"0\" step=\"1\" inputmode=\"numeric\" value=\"5\"></td><td><code>default</code></td>"
        ));
        assert!(html.contains("<span class=\"badge allow\">enforce</span>"));
        assert!(html.contains(&format!(
            "<code>{source}</code></th><td>project</td><td><span class=\"settings-trust-stale\">Trusted; changed on disk (stale)</span>"
        )));
        assert!(html.contains("Not present"), "global config is absent");
    }
}
