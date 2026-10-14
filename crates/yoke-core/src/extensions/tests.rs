use super::*;
use std::fs;

fn put(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn fixture() -> (tempfile::TempDir, Roots) {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let home = base.join("home");
    let project = base.join("project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&project).unwrap();
    let roots = Roots {
        claude_dir: home.join(".claude"),
        claude_json: home.join(".claude.json"),
        claude_managed: base.join("managed"),
        codex_home: home.join(".codex"),
        codex_system: base.join("etc-codex"),
        home,
        project,
    };
    (temp, roots)
}

fn find<'a>(report: &'a Report, kind: DriftKind, name: &str) -> &'a Drift {
    report
        .drift
        .iter()
        .find(|drift| drift.kind == kind && drift.name == name)
        .unwrap_or_else(|| panic!("no {kind:?} drift for {name}: {:#?}", report.drift))
}

#[test]
fn scan_resolves_documented_precedence_and_reports_links_dangling_and_stale_presets() {
    let (temp, roots) = fixture();
    let skill = "---\nname: deploy\ndescription: ship\n---\nbody";
    put(&roots.claude_dir.join("skills/deploy/SKILL.md"), skill);
    put(
        &roots.project.join(".claude/skills/release/SKILL.md"),
        skill,
    );
    put(&roots.project.join(".claude/commands/deploy.md"), "run it");
    put(
        &roots.claude_dir.join("agents/a.md"),
        "---\nname: reviewer\n---\n",
    );
    put(
        &roots.project.join(".claude/agents/b.md"),
        "---\nname: reviewer\n---\n",
    );
    put(&roots.codex_home.join("agents/a.toml"), "name = 'rev'\n");
    put(
        &roots.project.join(".codex/agents/b.toml"),
        "name = 'rev'\n",
    );
    let outside = temp.path().join("outside/shared");
    put(&outside.join("SKILL.md"), "---\nname: shared\n---\n");
    std::os::unix::fs::symlink(&outside, roots.claude_dir.join("skills/shared")).unwrap();
    put(
        &roots.project.join(".claude/settings.json"),
        r#"{"hooks":{"Stop":[{"hooks":[
            {"type":"command","command":"\"$CLAUDE_PROJECT_DIR\"/.claude/hooks/missing.sh --flag"},
            {"type":"command","command":"true"}]}]}}"#,
    );
    put(
        &roots.project.join("yoke.toml"),
        "# Yoke preset: git-safety\n[[gate]]\nname = 'preset-git-force-push'\n",
    );

    let report = scan(&roots);

    let deploy = find(&report, DriftKind::Shadowed, "deploy");
    assert_eq!(
        deploy.detail,
        "defined as project skill, user skill, project command: the user skill definition wins"
    );
    assert_eq!(deploy.origins.len(), 3);
    assert!(find(&report, DriftKind::Shadowed, "reviewer")
        .detail
        .ends_with("the project agent definition wins"));
    assert!(find(&report, DriftKind::Shadowed, "rev")
        .detail
        .ends_with("which definition wins is not documented"));
    let missing = find(&report, DriftKind::Dangling, "Stop");
    assert!(missing
        .detail
        .contains("$CLAUDE_PROJECT_DIR/.claude/hooks/missing.sh"));
    assert_eq!(
        report
            .drift
            .iter()
            .filter(|drift| drift.kind == DriftKind::Dangling)
            .count(),
        1
    );
    assert!(report.items.iter().any(|item| item.name == "shared"));
    assert!(report.notes.iter().any(|note| {
        note.path == roots.claude_dir.join("skills/shared")
            && note.note.starts_with("symlink outside scanned roots")
    }));
    let preset = find(&report, DriftKind::Stale, "git-safety");
    assert_eq!(preset.agent, Owner::Yoke);
}

#[test]
fn scan_shows_programs_origins_and_key_names_but_no_values() {
    let (_temp, roots) = fixture();
    let local = serde_json::json!({
        "mcpServers": {"db": {"command": "/bin/sh", "args": ["--token", "sk-live0123456789", "postgres://app:db-pass@db.local/main?sslkey=q-secret"], "env": {"API_KEY": "hunter2-value"}}},
        "projects": {roots.project.to_str().unwrap(): {"mcpServers": {"db": {
            "type": "http", "url": "https://user:pa55word@mcp.example/v1?key=query-secret",
            "headers": {"Authorization": "Bearer header-secret"}}}}}
    });
    put(&roots.claude_json, &local.to_string());
    put(
        &roots.codex_home.join("config.toml"),
        "[mcp_servers.docs]\ncommand = '/bin/sh'\nenv = { GITHUB_TOKEN = 'ghp_codexsecret' }\n[mcp_servers.bare]\nurl = 'https://admin:no-at-secret/sse'\n",
    );

    put(
        &roots.codex_home.join("hooks.json"),
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"curl -s https://bot:hook-pass@hooks.example/x"}]}]}}"#,
    );

    let report = scan(&roots);
    let text = serde_json::to_string(&report).unwrap();

    for secret in [
        "sk-live0123456789",
        "hunter2-value",
        "pa55word",
        "query-secret",
        "header-secret",
        "ghp_codexsecret",
        "db-pass",
        "q-secret",
        "hook-pass",
        "no-at-secret",
    ] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    let mcp: Vec<_> = report
        .items
        .iter()
        .filter(|item| item.kind == Kind::Mcp)
        .map(|item| (item.scope, item.keys.join(","), item.detail.as_str()))
        .collect();
    assert!(mcp.contains(&(Scope::User, "API_KEY".into(), "/bin/sh (3 args)")));
    assert!(mcp.contains(&(
        Scope::Local,
        "Authorization".into(),
        "http: https://mcp.example"
    )));
    assert!(mcp.contains(&(Scope::User, "GITHUB_TOKEN".into(), "/bin/sh (0 args)")));
    assert!(mcp.contains(&(Scope::User, String::new(), "unparsed URL")));
    assert!(report
        .items
        .iter()
        .any(|item| item.kind == Kind::Hook && item.detail == "curl (2 args)"));
    assert!(find(&report, DriftKind::Shadowed, "db")
        .detail
        .ends_with("the local mcp server definition wins"));
}
