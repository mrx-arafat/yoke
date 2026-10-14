//! Read-only workflow settings: what Yoke decides, where each value comes from, and what
//! stays with the native agent. Changes go through a config file and terminal trust.
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

fn snapshot_state(status: &SnapshotStatus) -> (&'static str, &'static str) {
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

    let model = |value: Option<&str>| value.unwrap_or("CLI default").to_owned();
    let rows: [(&str, String, &str); 13] = [
        ("mode", cfg.mode.as_str().into(), "observe records would-deny decisions; enforce lets gates deny, rewrite and block Stop."),
        ("review.reviewer", cfg.review.reviewer.clone(), "Agent CLI Yoke launches for independent review; auto picks the agent that is not building."),
        ("review.models.claude", model(cfg.review.models.get("claude")), "Model Yoke asks for when Claude Code reviews (--model)."),
        ("review.models.codex", model(cfg.review.models.get("codex")), "Model Yoke asks for when Codex reviews (-m)."),
        ("memory.digest", cfg.memory.digest.to_string(), "Whether Yoke launches an agent to digest session facts into memories."),
        ("memory.digest_agent", cfg.memory.digest_agent.clone(), "Agent CLI Yoke launches for the memory digest."),
        ("memory.digest_models.claude", model(cfg.memory.digest_models.get("claude")), "Model Yoke asks for when Claude Code digests memory."),
        ("memory.digest_models.codex", model(cfg.memory.digest_models.get("codex")), "Model Yoke asks for when Codex digests memory."),
        ("stop.max_blocks", cfg.stop.max_blocks.to_string(), "Stop attempts without progress before an active plan stalls."),
        ("plans.dir", cfg.plans.dir.clone(), "Where new plan files are created, relative to the project."),
        ("memory.inject_tokens", cfg.memory.inject_tokens.to_string(), "Token budget for memory context injected at session start in enforce mode."),
        ("log.retention_days", cfg.log.retention_days.to_string(), "Days of decisions, facts, finished check runs and notifications that are kept."),
        ("log.payload_cap", cfg.log.payload_cap.to_string(), "Bytes kept from each redacted hook payload."),
    ];
    out.push_str(r#"<section class="panel" aria-labelledby="settings-owned"><h2 id="settings-owned">Decided by Yoke</h2><div class="table-scroll" tabindex="0" role="region" aria-labelledby="settings-owned"><table><thead><tr><th scope="col">Setting</th><th scope="col">Effective value</th><th scope="col">Source</th><th scope="col">What it controls</th></tr></thead><tbody>"#);
    for (key, value, meaning) in rows {
        out.push_str(&format!(
            r#"<tr><th scope="row"><code>{}</code></th><td><code>{}</code></td><td><code>{}</code></td><td>{}</td></tr>"#,
            escape(key),
            escape(&value),
            escape(&origin(&loaded, key)),
            escape(meaning)
        ));
    }
    out.push_str("</tbody></table></div></section>");

    out.push_str(&format!(
        r#"<section class="panel" aria-labelledby="settings-native"><h2 id="settings-native">Decided by your coding agent</h2><p>Yoke does not choose or switch the planning and building model. Neither Claude Code nor Codex documents a way for an outside tool to change the model of a running session, so these stay native and Yoke only shows what transcripts report.</p><dl class="facts settings-facts"><dt>Claude Code</dt><dd><code>/model</code>, <code>claude --model</code>, <code>ANTHROPIC_MODEL</code> or the <code>model</code> setting. The <code>opusplan</code> alias uses Opus in plan mode and Sonnet otherwise.</dd><dt>Codex</dt><dd><code>codex --model</code> (<code>-m</code>) or <code>model</code> in <code>~/.codex/config.toml</code>.</dd><dt>Reviewer and digest</dt><dd>Yoke launches these itself, so it passes the models above. Unset means the CLI default; Yoke never adds a fallback model, so an unavailable model fails with the CLI error.</dd></dl><p><a data-nav href="{}">See the models recorded in transcripts</a></p></section>"#,
        render::link(c, "usage")
    ));
    out.push_str(r#"<p class="muted">This page is read-only. Edit <code>yoke.toml</code> or <code>~/.yoke/config.toml</code>, then run <code>yoke trust</code> or <code>yoke trust --global</code> in your own terminal. Evaluation: <code>docs/decisions/0002-model-routing.md</code>.</p></main>"#);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_show_effective_values_origins_and_stale_trust() {
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
        assert!(html.contains(&format!(
            "<th scope=\"row\"><code>review.reviewer</code></th><td><code>codex</code></td><td><code>{source}</code></td>"
        )));
        assert!(html.contains(
            "<th scope=\"row\"><code>stop.max_blocks</code></th><td><code>5</code></td><td><code>default</code></td>"
        ));
        assert!(html.contains("<span class=\"badge allow\">enforce</span>"));
        assert!(html.contains(&format!(
            "<code>{source}</code></th><td>project</td><td><span class=\"settings-trust-stale\">Trusted; changed on disk (stale)</span>"
        )));
        assert!(html.contains("Not present"), "global config is absent");
    }
}
