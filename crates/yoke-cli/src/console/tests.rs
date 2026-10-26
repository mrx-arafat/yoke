use super::*;

#[test]
fn every_route_requires_session_cookie_host_and_same_origin_mutations() {
    let c = Console {
        home: PathBuf::new(),
        project: PathBuf::new(),
        database: PathBuf::new(),
        token: "secret".into(),
        csrf: "csrf".into(),
        host: "127.0.0.1:1234".into(),
    };
    let mut headers = BTreeMap::from([("host".into(), c.host.clone())]);
    assert!(!authorize(&c, "GET", &headers));
    headers.insert("authorization".into(), "Bearer secret".into());
    assert!(
        !authorize(&c, "GET", &headers),
        "only the cookie authenticates"
    );
    headers.insert("cookie".into(), "yoke_9999=secret".into());
    assert!(!authorize(&c, "GET", &headers), "another console's cookie");
    headers.insert("cookie".into(), "theme=dark; yoke_1234=secret".into());
    assert!(authorize(&c, "GET", &headers));
    assert!(!authorize(&c, "POST", &headers));
    headers.insert("origin".into(), "http://evil.invalid".into());
    headers.insert("x-yoke-csrf".into(), "csrf".into());
    headers.insert("content-type".into(), "application/json".into());
    assert!(!authorize(&c, "POST", &headers));
    headers.insert("origin".into(), format!("http://{}", c.host));
    assert!(authorize(&c, "POST", &headers));
    headers.insert("sec-fetch-site".into(), "same-site".into());
    assert!(!authorize(&c, "POST", &headers));
    headers.remove("sec-fetch-site");
    headers.insert("host".into(), "evil.invalid".into());
    assert!(!authorize(&c, "GET", &headers));
    assert!(query("/decisions?token=a&token=b").is_err());
}

#[test]
fn memory_actions_scope_ids_and_undo_only_once() {
    use yoke_core::memory;
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().canonicalize().unwrap();
    let database = project.join("test.db");
    let store = Store::open(&database).unwrap();
    let c = Console {
        home: project.clone(),
        project: project.clone(),
        database,
        token: "token".into(),
        csrf: "csrf".into(),
        host: "127.0.0.1:1".into(),
    };
    let m = memory::remember(
        &store,
        &project.to_string_lossy(),
        "Keep source provenance",
        false,
        1,
    )
    .unwrap();
    let mut actions = actions::Actions::default();
    let apply = |actions: &mut actions::Actions, value| {
        actions.apply(&store, &c, serde_json::from_value(value).unwrap())
    };
    apply(
        &mut actions,
        serde_json::json!({"action":"pin","id":m.id,"value":true}),
    )
    .unwrap();
    assert!(memory::get(&store, m.id).unwrap().unwrap().pinned);
    apply(
        &mut actions,
        serde_json::json!({"action":"share","id":m.id}),
    )
    .unwrap();
    let result = apply(
        &mut actions,
        serde_json::json!({"action":"forget","id":m.id}),
    )
    .unwrap();
    assert!(memory::get(&store, m.id).unwrap().is_none());
    assert!(result["data"]["message"]
        .as_str()
        .unwrap()
        .contains(".yoke/knowledge.md"));
    assert!(std::fs::read_to_string(project.join(".yoke/knowledge.md"))
        .unwrap()
        .contains("Keep source provenance"));
    let undo = serde_json::json!({"action":"undo-forget","ticket":result["data"]["undo"]});
    apply(&mut actions, undo.clone()).unwrap();
    assert_eq!(
        memory::get(&store, m.id).unwrap().unwrap().identity,
        m.identity
    );
    assert!(apply(&mut actions, undo).is_err());
    let other = memory::remember(
        &store,
        &project.join("other").to_string_lossy(),
        "Other project",
        false,
        2,
    )
    .unwrap();
    assert!(apply(
        &mut actions,
        serde_json::json!({"action":"forget","id":other.id})
    )
    .is_err());
    assert!(memory::get(&store, other.id).unwrap().is_some());
}

fn oversight_fixture(temp: &std::path::Path) -> (Store, Console, String) {
    use yoke_core::store::DecisionRecord;
    let launch = temp.join("alpha");
    let other = temp.join("beta");
    for dir in [&launch, &other] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let (launch, other) = (
        launch.canonicalize().unwrap(),
        other.canonicalize().unwrap(),
    );
    let database = temp.join("state.db");
    let store = Store::open(&database).unwrap();
    for (session, project, model) in [
        ("session-alpha", &launch, "model-alpha"),
        ("session-beta", &other, "model-beta"),
    ] {
        let project = project.to_string_lossy().into_owned();
        store
            .record_decision(
                &DecisionRecord {
                    policy_hash: None,
                    ts: 1_760_000_000,
                    session_id: session.into(),
                    agent: "claude".into(),
                    project: project.clone(),
                    event: "pre_tool_use".into(),
                    tool: Some("Bash".into()),
                    mode: "enforce".into(),
                    outcome: "allow".into(),
                    reason: None,
                    gates: serde_json::json!([]),
                    payload: "{}".into(),
                },
                4096,
            )
            .unwrap();
        let transcript = temp.join(format!("{session}.jsonl"));
        let row = serde_json::json!({"type":"assistant","sessionId":session,"timestamp":"2026-10-13T18:30:00Z","message":{"id":"m","model":model,"usage":{"input_tokens":7,"output_tokens":3}}});
        std::fs::write(&transcript, format!("{row}\n")).unwrap();
        yoke_core::usage::ingest(&store, "claude", session, &project, &transcript).unwrap();
    }
    // A hook payload that failed to parse records its decision under an empty session id.
    let mut unparsed = store
        .decisions(&yoke_core::store::DecisionFilter::default())
        .unwrap()
        .into_iter()
        .find(|row| row.session_id == "session-alpha")
        .unwrap();
    unparsed.session_id = String::new();
    unparsed.outcome = "error".into();
    store.record_decision(&unparsed, 4096).unwrap();
    let c = Console {
        home: temp.to_path_buf(),
        project: launch,
        database,
        token: "token".into(),
        csrf: "csrf".into(),
        host: "127.0.0.1:1".into(),
    };
    (store, c, projects::project_id(&other.to_string_lossy()))
}

#[test]
fn decisions_show_safe_previews_and_reasons_before_expanding() {
    let temp = tempfile::tempdir().unwrap();
    let (store, c, _) = oversight_fixture(temp.path());
    let mut row = store
        .decisions(&yoke_core::store::DecisionFilter::default())
        .unwrap()
        .remove(0);
    row.project = c.project.to_string_lossy().into_owned();
    row.outcome = "deny".into();
    row.reason = Some("Protected <branch> requires review".into());
    for (agent, input, expected) in [
        (
            "claude",
            serde_json::json!({"command": "echo '<img src=x onerror=alert(1)>' --token hunter2"}),
            "echo &#39;&lt;img src=x onerror=alert(1)&gt;&#39; --token [REDACTED]",
        ),
        (
            "codex",
            serde_json::json!({"file_path": "src/<unsafe>.rs"}),
            "src/&lt;unsafe&gt;.rs",
        ),
        (
            "codex",
            serde_json::json!({"cmd": "echo '<unsafe>'"}),
            "echo &#39;&lt;unsafe&gt;&#39;",
        ),
        (
            "codex",
            serde_json::json!({"cmd": "界".repeat(240)}),
            &format!("{}…", "界".repeat(180)),
        ),
        (
            "claude",
            serde_json::json!({"command": "echo\n\t<unsafe>"}),
            "echo  &lt;unsafe&gt;",
        ),
    ] {
        row.agent = agent.into();
        row.payload = serde_json::json!({"tool_input": input}).to_string();
        store.record_decision(&row, 4096).unwrap();
        let html = pages::decisions(&store, &c, &BTreeMap::new()).unwrap();
        let summary = html
            .split("<summary data-row>")
            .nth(1)
            .unwrap()
            .split("</summary>")
            .next()
            .unwrap();
        assert!(summary.contains(expected), "{summary}");
        assert!(summary.contains("Protected &lt;branch&gt; requires review"));
        assert!(!summary.contains("<img"));
        assert!(!summary.contains("hunter2"));
        assert!(!summary.contains(&"界".repeat(181)));
    }
    // Payloads are byte-capped at persistence and need not remain valid JSON.
    row.payload = "{\"tool_input\":".into();
    row.reason = None;
    store.record_decision(&row, 4096).unwrap();
    let html = pages::decisions(&store, &c, &BTreeMap::new()).unwrap();
    assert!(html.contains("No reason was recorded."));
    let first = html.split("<div class=\"record-detail\">").nth(1).unwrap();
    assert!(first.starts_with("<p class=\"reason\">No reason was recorded.</p>"));
    let first = html
        .split("<summary data-row>")
        .nth(1)
        .unwrap()
        .split("</summary>")
        .next()
        .unwrap();
    assert!(!first.contains("decision-preview"));
}

#[test]
fn decisions_empty_states_distinguish_filters_history_and_new_projects() {
    let temp = tempfile::tempdir().unwrap();
    let (store, mut c, beta) = oversight_fixture(temp.path());
    let params = BTreeMap::from([
        ("outcome".into(), "deny".into()),
        ("project".into(), beta.clone()),
    ]);
    let html = pages::decisions(&store, &c, &params).unwrap();
    assert!(html.contains("No decisions match these filters"));
    assert!(html.contains(&format!(
        "href=\"decisions?project={beta}\">Clear filters</a>"
    )));
    assert!(!html.contains("yoke install"));
    let params = BTreeMap::from([("before".into(), "0".into())]);
    let html = pages::decisions(&store, &c, &params).unwrap();
    assert!(html.contains("No older decisions"));
    assert!(html.contains("Latest decisions"));
    c.project = temp.path().join("new-project");
    let html = pages::decisions(&store, &c, &BTreeMap::new()).unwrap();
    assert!(html.contains("No decisions here yet"));
    assert!(html.contains("yoke install --dry-run"));
    let params = BTreeMap::from([("session".into(), "missing-session".into())]);
    assert!(pages::decisions(&store, &c, &params)
        .unwrap()
        .contains("No decisions match these filters"));
}

#[test]
fn project_scoped_pages_never_show_another_projects_records() {
    let temp = tempfile::tempdir().unwrap();
    let (store, c, beta) = oversight_fixture(temp.path());
    let launch = BTreeMap::new();
    let scoped = BTreeMap::from([("project".to_owned(), beta.clone())]);
    let pages = |params: &BTreeMap<String, String>| {
        [
            views::sessions(&store, &c, params).unwrap(),
            pages::decisions(&store, &c, params).unwrap(),
            usage::page(&store, &c, params).unwrap(),
        ]
    };
    for html in pages(&launch) {
        assert!(html.contains("session-alpha") && !html.contains("session-beta"));
    }
    for html in pages(&scoped) {
        assert!(html.contains("session-beta") && !html.contains("session-alpha"));
        assert!(html.contains("Back to launch project"));
    }
    let sessions = views::sessions(&store, &c, &launch).unwrap();
    assert!(sessions.contains("<h2>No session id recorded</h2>"));
    assert!(sessions.contains(">alpha (launch project)</option>") && !sessions.contains("(/"));
    assert!(sessions.contains("claude --resume &#39;session-alpha&#39;"));
    assert!(sessions.contains("Mode at last event</dt><dd>enforce"));
    assert!(sessions.contains("no end event recorded"));
    let overview = projects::overview(&store, &c).unwrap();
    assert!(overview.contains("alpha") && overview.contains("beta"));
    assert!(overview.contains(&format!("project={beta}")));
    let filtered = BTreeMap::from([("session".to_owned(), "session-alpha".to_owned())]);
    let rendered = [
        pages(&launch).to_vec(),
        pages(&scoped).to_vec(),
        pages(&filtered).to_vec(),
        vec![overview.clone(), render::shell(&c, "decisions", "", 2)],
    ]
    .concat();
    for html in &rendered {
        for attribute in ["href=\"", "data-destination=\"", "data-image=\""] {
            for url in html
                .split(attribute)
                .skip(1)
                .map(|rest| rest.split('"').next().unwrap())
            {
                assert!(
                    !url.contains("?&") && !url.ends_with('?'),
                    "malformed link {url}"
                );
            }
        }
    }
    for forged in ["/etc", "../beta", "0000000000000000", "%zz"] {
        let params = BTreeMap::from([("project".to_owned(), forged.to_owned())]);
        for result in [
            views::sessions(&store, &c, &params),
            pages::decisions(&store, &c, &params),
            usage::page(&store, &c, &params),
        ] {
            assert_eq!(view_status(&result.unwrap_err()), 400);
        }
    }
    assert_eq!(view_status(&anyhow::anyhow!("database unavailable")), 500);
    assert!(projects::resume_command("claude", "/p", "id; rm -rf /").is_none());
}

#[test]
fn usage_page_labels_offsets_keeps_unknown_prices_and_rejects_bad_ranges() {
    let temp = tempfile::tempdir().unwrap();
    let (store, c, _) = oversight_fixture(temp.path());
    let params = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    let utc = usage::page(&store, &c, &params(&[("tz", "+00:00")])).unwrap();
    assert!(utc.contains("<th scope=\"row\">2026-10-13</th>"));
    let dhaka = usage::page(&store, &c, &params(&[("tz", "+06:00")])).unwrap();
    assert!(dhaka.contains("calendar days in <strong>UTC+06:00</strong>"));
    assert!(dhaka.contains("<th scope=\"row\">2026-10-14</th>"));
    assert!(dhaka.contains("Unknown (1 sample)"));
    for bad in [
        params(&[("from", "2026-02-30")]),
        params(&[("from", "2026-10-14"), ("to", "2026-10-13")]),
        params(&[("tz", "+06:07")]),
        params(&[("tz", "06:00")]),
    ] {
        let html = usage::page(&store, &c, &bad).unwrap();
        assert!(html.contains("role=\"alert\"") && !html.contains("<h2>Totals</h2>"));
    }
    assert_eq!(
        usage::cost(None, Some("0.000150000"), 2),
        "0.000150000 + unknown (2 samples)"
    );
}

#[test]
fn session_secret_never_reaches_rendered_html_links_or_forms() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().canonicalize().unwrap();
    let database = project.join("test.db");
    let store = Store::open(&database).unwrap();
    let c = Console {
        home: project.clone(),
        project: project.clone(),
        database,
        token: "a1b2c3d4e5f6-session-secret".into(),
        csrf: "csrf".into(),
        host: "127.0.0.1:1".into(),
    };
    let params = BTreeMap::new();
    for body in [
        pages::decisions(&store, &c, &params).unwrap(),
        views::sessions(&store, &c, &params).unwrap(),
        views::memories(&store, &c, &params).unwrap(),
        usage::page(&store, &c, &params).unwrap(),
        notifications::page(&store, &c, &params).unwrap(),
    ] {
        let html = render::shell(&c, "decisions", &body, 0);
        assert!(!html.contains(&c.token), "{html}");
        assert!(!html.contains("?token=") && !html.contains(r#"name="token""#));
    }
}

#[test]
fn trust_page_shows_the_trusted_bytes_after_the_disk_file_is_deleted() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (home, project) = (root.join("home"), root.join("project"));
    std::fs::create_dir_all(home.join(".yoke")).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    let store = Store::open(home.join(".yoke/state.db")).unwrap();
    let file = project.join("yoke.toml");
    std::fs::write(&file, "mode = 'enforce'\n").unwrap();
    yoke_core::trust::prepare_project(&store, &project)
        .unwrap()
        .commit(&store)
        .unwrap();
    std::fs::remove_file(&file).unwrap();
    let c = Console {
        home,
        project,
        database: root.join("unused.db"),
        token: "token".into(),
        csrf: "csrf".into(),
        host: "127.0.0.1:1".into(),
    };
    let html = views::trust(&store, &c).unwrap();
    assert!(
        html.contains("<p>project / Trusted; missing or unreadable on disk (stale)</p>"),
        "{html}"
    );
    assert!(
        html.contains("The trusted content below still applies.</p><pre><code>mode = <span class=\"token-string\">&#39;enforce&#39;</span>"),
        "{html}"
    );
    assert!(!html.contains("Disk changed"));
    assert!(html.contains("<p>global / Not present</p>"), "{html}");
}

/// Drives `serve` through the real loopback transport, as a browser would.
struct Live {
    server: http::Server,
    host: String,
    c: Console,
    store: Store,
    actions: actions::Actions,
    connections: Arc<AtomicUsize>,
}

impl Live {
    fn new(temp: &std::path::Path) -> Self {
        let project = temp.canonicalize().unwrap();
        let database = project.join("state.db");
        let store = Store::open(&database).unwrap();
        let server = http::Server::bind_loopback().unwrap();
        let host = server.local_addr().unwrap().to_string();
        let c = Console {
            home: project.clone(),
            project,
            database,
            token: random_token().unwrap(),
            csrf: random_token().unwrap(),
            host: host.clone(),
        };
        Self {
            server,
            host,
            c,
            store,
            actions: actions::Actions::default(),
            connections: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn port(&self) -> &str {
        self.host.rsplit(':').next().unwrap()
    }

    /// One request and its full response; `extra` are header lines without CRLF.
    fn exchange(&mut self, method: &str, target: &str, extra: &[String], body: &[u8]) -> String {
        use std::io::{Read, Write};
        let mut client = std::net::TcpStream::connect(&self.host).unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut raw = format!("{method} {target} HTTP/1.1\r\n");
        if !extra
            .iter()
            .any(|line| line.to_ascii_lowercase().starts_with("host:"))
        {
            raw.push_str(&format!("Host: {}\r\n", self.host));
        }
        for line in extra {
            raw.push_str(line);
            raw.push_str("\r\n");
        }
        if method == "POST" {
            raw.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        raw.push_str("\r\n");
        client.write_all(raw.as_bytes()).unwrap();
        client.write_all(body).unwrap();
        let request = self.server.recv().unwrap();
        let _ = serve(
            &self.c,
            &self.store,
            &mut self.actions,
            &self.connections,
            request,
        );
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        response
    }
}

#[test]
fn decisions_link_to_escaped_historical_policy_without_reading_current_files() {
    let temp = tempfile::tempdir().unwrap();
    let mut live = Live::new(temp.path());
    let file = live.c.project.join("yoke.toml");
    let first = "mode='enforce'\n# <script>alert(1)</script>\n";
    std::fs::write(&file, first).unwrap();
    yoke_core::trust::prepare_project(&live.store, &live.c.project)
        .unwrap()
        .commit(&live.store)
        .unwrap();
    let loaded =
        yoke_core::config::load_trusted(&live.store, &live.c.project, &live.c.home).unwrap();
    let hash = live.store.record_policy_version(&loaded).unwrap();
    let mut row = yoke_core::store::DecisionRecord {
        policy_hash: None,
        ts: unix_now(),
        session_id: "history".into(),
        agent: "claude".into(),
        project: live.c.project.to_string_lossy().into_owned(),
        event: "pre_tool_use".into(),
        tool: Some("Bash".into()),
        mode: "enforce".into(),
        outcome: "deny".into(),
        reason: Some("Denied by old policy".into()),
        gates: serde_json::json!([]),
        payload: "{}".into(),
    };
    live.store.record_decision(&row, 4096).unwrap();
    row.policy_hash = Some(hash.clone());
    live.store.record_decision(&row, 4096).unwrap();
    std::fs::write(&file, "mode='observe'\n# new policy\n").unwrap();
    yoke_core::trust::prepare_project(&live.store, &live.c.project)
        .unwrap()
        .commit(&live.store)
        .unwrap();
    std::fs::remove_file(&file).unwrap();
    let base = launch::base(&live.c);
    let cookie = vec![format!("Cookie: yoke_{}={}", live.port(), live.c.token)];
    let decisions = live.exchange("GET", &format!("{base}decisions"), &cookie, b"");
    assert!(
        decisions.contains(&format!("href=\"policy?hash={hash}\"")),
        "{decisions}"
    );
    assert!(decisions.contains("Policy version was not recorded"));
    let policy = live.exchange("GET", &format!("{base}policy?hash={hash}"), &cookie, b"");
    assert!(policy.starts_with("HTTP/1.1 200 "), "{policy}");
    assert!(policy.contains("Historical policy"));
    assert!(policy.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(!policy.contains("<script>alert(1)"));
    assert!(!policy.contains("new policy"));
    let missing = live.exchange(
        "GET",
        &format!("{base}policy?hash={}", "0".repeat(64)),
        &cookie,
        b"",
    );
    assert!(missing.starts_with("HTTP/1.1 404 "));
    let invalid = live.exchange(
        "GET",
        &format!("{base}policy?hash=../../etc/passwd"),
        &cookie,
        b"",
    );
    assert!(invalid.starts_with("HTTP/1.1 400 "));
    let unsigned = live.exchange("GET", &format!("{base}policy?hash={hash}"), &[], b"");
    assert!(unsigned.starts_with("HTTP/1.1 403 "));
}

#[test]
fn serve_rejects_foreign_hosts_traversal_and_unsafe_posts_and_sets_security_headers() {
    let temp = tempfile::tempdir().unwrap();
    let mut live = Live::new(temp.path());
    let base = launch::base(&live.c);
    let port = live.port().to_owned();
    let cookie = format!("Cookie: yoke_{port}={}", live.c.token);
    let secure = |response: &str| {
        let head = response.split("\r\n\r\n").next().unwrap().to_owned();
        for required in [
            "Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'\r\n",
            "X-Content-Type-Options: nosniff\r\n",
            "X-Frame-Options: DENY\r\n",
            "Cache-Control: no-store\r\n",
            "Referrer-Policy: no-referrer\r\n",
        ] {
            assert!(head.contains(required), "missing {required:?} in {head}");
        }
        assert!(!head.contains("Set-Cookie"), "{head}");
    };
    // DNS rebinding: a foreign Host never reaches a page, even with the session cookie.
    let (local, spoof) = (
        format!("localhost:{port}"),
        format!("127.0.0.1.evil.example:{port}"),
    );
    for host in ["evil.example", local.as_str(), spoof.as_str()] {
        let response = live.exchange(
            "GET",
            &format!("{base}decisions"),
            &[format!("Host: {host}"), cookie.clone()],
            b"",
        );
        assert!(response.starts_with("HTTP/1.1 403 "), "{host}: {response}");
        secure(&response);
    }
    // Path handling: no route is file-backed, so traversal in any spelling is an unknown page.
    for extra in [
        vec![cookie.clone(), "Origin: https://evil.example".into()],
        vec![cookie.clone(), "Origin: null".into()],
        vec![cookie.clone(), "Sec-Fetch-Site: cross-site".into()],
        vec![cookie.clone(), "Sec-Fetch-Site: same-site".into()],
    ] {
        let response = live.exchange("GET", &format!("{base}decisions"), &extra, b"");
        assert!(
            response.starts_with("HTTP/1.1 403 "),
            "{extra:?}: {response}"
        );
        secure(&response);
    }
    for target in [
        format!("{base}../assets/console.css"),
        format!("{base}%2e%2e/%2e%2e/etc/passwd"),
        format!("{base}assets\\..\\console.css"),
        format!("{base}assets/../../etc/passwd"),
        format!("{base}decisions/../assets/console.css"),
        format!("{base}./decisions"),
        format!("{base}Decisions"),
        format!("{base}assets/console.css/.."),
    ] {
        let response = live.exchange("GET", &target, std::slice::from_ref(&cookie), b"");
        assert!(
            response.starts_with("HTTP/1.1 404 ") && response.ends_with("Page not found"),
            "{target}: {response}"
        );
    }
    // Every page carries the hardening headers and hands console.js its CSRF token.
    let response = live.exchange(
        "GET",
        &format!("{base}decisions"),
        std::slice::from_ref(&cookie),
        b"",
    );
    assert!(response.starts_with("HTTP/1.1 200 "), "{response}");
    assert!(response.contains("Content-Type: text/html; charset=utf-8\r\n"));
    assert!(response.contains(&format!(
        r#"<meta name="yoke-csrf" content="{}">"#,
        live.c.csrf
    )));
    secure(&response);
    // The event stream sits behind the same session check and never opens a slot for strangers.
    let response = live.exchange("GET", &format!("{base}events"), &[], b"");
    assert!(response.starts_with("HTTP/1.1 403 "), "{response}");
    assert_eq!(
        live.connections.load(std::sync::atomic::Ordering::Acquire),
        0
    );
    // POST needs the cookie, the exact Origin, the CSRF header and JSON, on the action route only.
    let origin = format!("Origin: http://{}", live.host);
    let csrf = format!("X-Yoke-CSRF: {}", live.c.csrf);
    let json = "Content-Type: application/json".to_owned();
    let body = br#"{"action":"approve","ticket":"nope"}"#;
    let actions = format!("{base}actions");
    for (name, extra) in [
        (
            "no cookie",
            vec![origin.clone(), csrf.clone(), json.clone()],
        ),
        (
            "no csrf",
            vec![cookie.clone(), origin.clone(), json.clone()],
        ),
        (
            "wrong csrf",
            vec![
                cookie.clone(),
                origin.clone(),
                json.clone(),
                "X-Yoke-CSRF: csrf".into(),
            ],
        ),
        (
            "foreign origin",
            vec![
                cookie.clone(),
                format!("Origin: http://localhost:{port}"),
                csrf.clone(),
                json.clone(),
            ],
        ),
        (
            "null origin",
            vec![
                cookie.clone(),
                "Origin: null".into(),
                csrf.clone(),
                json.clone(),
            ],
        ),
        (
            "no origin",
            vec![cookie.clone(), csrf.clone(), json.clone()],
        ),
        (
            "form content type",
            vec![
                cookie.clone(),
                origin.clone(),
                csrf.clone(),
                "Content-Type: application/x-www-form-urlencoded".into(),
            ],
        ),
        (
            "cross-site fetch metadata",
            vec![
                cookie.clone(),
                origin.clone(),
                csrf.clone(),
                json.clone(),
                "Sec-Fetch-Site: cross-site".into(),
            ],
        ),
    ] {
        let response = live.exchange("POST", &actions, &extra, body);
        assert!(response.starts_with("HTTP/1.1 403 "), "{name}: {response}");
        secure(&response);
    }
    let full = vec![cookie.clone(), origin.clone(), csrf.clone(), json.clone()];
    let response = live.exchange("POST", &format!("{base}decisions"), &full, body);
    assert!(response.starts_with("HTTP/1.1 404 "), "{response}");
    let response = live.exchange("POST", &actions, &full, body);
    assert!(response.starts_with("HTTP/1.1 409 "), "{response}");
    assert!(response.contains("Content-Type: application/json\r\n"));
    secure(&response);
    let payload: serde_json::Value =
        serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(payload["error"]["code"], "action_failed");
    assert_eq!(
        payload["error"]["message"],
        "Approval expired or already used; review the current header again."
    );
    // The action body cap applies before any parsing.
    let huge = format!(r#"{{"action":"{}"}}"#, "a".repeat(16400));
    let response = live.exchange("POST", &actions, &full, huge.as_bytes());
    assert!(
        response.starts_with("HTTP/1.1 409 ") && response.contains("Action request too large"),
        "{response}"
    );
    // A non-UTF-8 action body gets an answer instead of a dropped connection.
    let response = live.exchange("POST", &actions, &full, &[0xff, 0xfe]);
    assert!(
        response.starts_with("HTTP/1.1 409 ") && response.contains("Action body must be UTF-8"),
        "{response}"
    );
    // Reflected request content passes through redaction before it is echoed.
    let leak = br#"{"action":"approve","ghp_0123456789abcdefghijklmnop":1}"#;
    let response = live.exchange("POST", &actions, &full, leak);
    assert!(
        response.starts_with("HTTP/1.1 409 ")
            && response.contains("[REDACTED]")
            && !response.contains("ghp_0123"),
        "{response}"
    );
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[test]
fn handler_mutations_require_csrf_and_artifact_queries_cannot_choose_files() {
    use yoke_core::{memory, plan};
    let temp = tempfile::tempdir().unwrap();
    let mut live = Live::new(temp.path());
    let base = launch::base(&live.c);
    let cookie = format!("Cookie: yoke_{}={}", live.port(), live.c.token);
    let note = memory::remember(
        &live.store,
        &live.c.project.to_string_lossy(),
        "local note",
        false,
        1,
    )
    .unwrap();
    let body = serde_json::json!({"action":"pin","id":note.id,"value":true}).to_string();
    let mut headers = vec![
        cookie,
        format!("Origin: http://{}", live.host),
        "Content-Type: application/json".into(),
    ];
    let response = live.exchange("POST", &format!("{base}actions"), &headers, body.as_bytes());
    assert!(response.starts_with("HTTP/1.1 403 "));
    assert!(!memory::get(&live.store, note.id).unwrap().unwrap().pinned);
    headers.push(format!("X-Yoke-CSRF: {}", live.c.csrf));
    let response = live.exchange("POST", &format!("{base}actions"), &headers, body.as_bytes());
    assert!(response.starts_with("HTTP/1.1 200 "), "{response}");
    assert!(memory::get(&live.store, note.id).unwrap().unwrap().pinned);

    let created = plan::create(
        &live.store,
        &live.c.project,
        ".yoke/plans",
        "images",
        "Images",
    )
    .unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("private.png");
    std::fs::write(&secret, b"private image content").unwrap();
    let link = live.c.project.join("linked.png");
    std::os::unix::fs::symlink(&secret, &link).unwrap();
    let fake = live.c.project.join("fake.png");
    std::fs::write(&fake, b"<script>not an image</script>").unwrap();
    let artifacts = serde_json::json!([
        secret.to_str().unwrap(),
        link.to_str().unwrap(),
        fake.to_str().unwrap()
    ]);
    let connection = rusqlite::Connection::open(&live.c.database).unwrap();
    connection.execute(
        "INSERT INTO check_runs(plan_id,ts,name,command,output_tail,fingerprint,blocking,ms,result,artifacts) VALUES(?1,1,'image','true','','test',1,1,'pass',?2)",
        rusqlite::params![created.id, artifacts.to_string()],
    ).unwrap();
    let check = connection.last_insert_rowid();
    for index in ["0", "1", "2", "3", "-1", "184467440737095516160"] {
        let response = live.exchange(
            "GET",
            &format!("{base}artifact?slug=images&check={check}&index={index}"),
            &headers[..1],
            b"",
        );
        assert!(response.starts_with("HTTP/1.1 404 "), "{index}: {response}");
        assert!(!response.contains("private image content") && !response.contains("<script>"));
    }
    let response = live.exchange(
        "GET",
        &format!(
            "{base}artifact?slug=../images&check={check}&index=0&path={}",
            render::encode(secret.to_str().unwrap())
        ),
        &headers[..1],
        b"",
    );
    assert!(response.starts_with("HTTP/1.1 404 "), "{response}");
}

#[test]
fn hostile_records_render_escaped_and_secrets_redacted_on_every_page() {
    use yoke_core::{memory, plan, store::DecisionRecord};
    const SCRIPT: &str = "<script>alert(1)</script>";
    const BREAKOUT: &str = "\"><img src=x onerror=alert(2)>";
    const CLOSER: &str = "</script><script>alert(3)</script>";
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("home");
    std::fs::create_dir_all(home.join(".yoke")).unwrap();
    // The project folder name is hostile too: it reaches titles, pickers and resume commands.
    let project = root.join(BREAKOUT);
    std::fs::create_dir_all(&project).unwrap();
    let project_str = project.to_string_lossy().into_owned();
    let database = home.join(".yoke/state.db");
    let store = Store::open(&database).unwrap();
    let now = unix_now();
    store
        .record_decision(
            &DecisionRecord {
                policy_hash: None,
                ts: now,
                session_id: CLOSER.into(),
                agent: "claude".into(),
                project: project_str.clone(),
                event: "pre_tool_use".into(),
                tool: Some(BREAKOUT.into()),
                mode: "enforce".into(),
                outcome: "error".into(),
                reason: Some(format!("{SCRIPT} token=sk-live-0123456789")),
                gates: serde_json::json!([{"gate": SCRIPT, "api_key": "hunter2-gate"}]),
                payload: serde_json::json!({"command": SCRIPT, "authorization": "Bearer hunter2-payload"})
                    .to_string(),
            },
            4096,
        )
        .unwrap();
    // One memory is a CommonMark HTML block, the other carries inline HTML, a link and an image.
    for text in [
        SCRIPT.to_owned(),
        format!(
            "Note {SCRIPT} [x](javascript:alert(4)) ![y](https://evil.invalid/a.png) {BREAKOUT}"
        ),
    ] {
        memory::remember(&store, &project_str, &text, false, now).unwrap();
    }
    let transcript = root.join("transcript.jsonl");
    let row = serde_json::json!({"type":"assistant","sessionId":CLOSER,"timestamp":"2026-10-13T18:30:00Z","message":{"id":"m","model":BREAKOUT,"usage":{"input_tokens":7,"output_tokens":3}}});
    std::fs::write(&transcript, format!("{row}\n")).unwrap();
    yoke_core::usage::ingest(&store, "claude", CLOSER, &project_str, &transcript).unwrap();
    let created = plan::create(&store, &project, ".yoke/plans", "hostile", "Hostile").unwrap();
    std::fs::write(
        &created.file,
        format!("+++\ntitle='{SCRIPT}'\n[[check]]\nname='diff'\nrun='git diff --check'\n+++\n\n# Body\n{BREAKOUT}\n[x](javascript:alert(4))\n"),
    )
    .unwrap();
    plan::comment(
        &store,
        &project,
        "hostile",
        "plan",
        &format!("{SCRIPT} {BREAKOUT}"),
        plan::Actor::Agent,
    )
    .unwrap();
    let c = Console {
        home,
        project: project.clone(),
        database,
        token: random_token().unwrap(),
        csrf: "csrf".into(),
        host: "127.0.0.1:1".into(),
    };
    let mut actions = actions::Actions::default();
    let confirmation = actions
        .apply(
            &store,
            &c,
            serde_json::from_value(
                serde_json::json!({"action":"prepare-approval","slug":"hostile"}),
            )
            .unwrap(),
        )
        .unwrap()["data"]["confirmation"]
        .as_str()
        .unwrap()
        .to_owned();
    let params = BTreeMap::new();
    let detail = BTreeMap::from([("slug".to_owned(), "hostile".to_owned())]);
    let escaped_script = "&lt;script&gt;alert(1)&lt;/script&gt;";
    let escaped_breakout = "&quot;&gt;&lt;img src=x onerror=alert(2)&gt;";
    let escaped_closer = "&lt;/script&gt;&lt;script&gt;alert(3)&lt;/script&gt;";
    let rendered = [
        (
            "decisions",
            pages::decisions(&store, &c, &params).unwrap(),
            escaped_script,
        ),
        (
            "sessions",
            views::sessions(&store, &c, &params).unwrap(),
            escaped_closer,
        ),
        (
            "plans",
            views::plans(&store, &c, &params).unwrap(),
            "hostile",
        ),
        (
            "plan detail",
            views::plans(&store, &c, &detail).unwrap(),
            escaped_script,
        ),
        (
            "memory",
            views::memories(&store, &c, &params).unwrap(),
            escaped_script,
        ),
        (
            "usage",
            usage::page(&store, &c, &params).unwrap(),
            escaped_breakout,
        ),
        (
            "notifications",
            notifications::page(&store, &c, &params).unwrap(),
            escaped_script,
        ),
        (
            "projects",
            projects::overview(&store, &c).unwrap(),
            escaped_breakout,
        ),
        ("settings", settings::page(&store, &c).unwrap(), "settings"),
        ("trust", views::trust(&store, &c).unwrap(), "trust"),
        (
            "shell",
            render::shell(&c, "decisions", "", 0),
            escaped_breakout,
        ),
        ("approval dialog", confirmation, escaped_script),
    ];
    for (name, html, expected) in &rendered {
        assert!(
            html.contains(expected),
            "{name} skipped the hostile record: {html}"
        );
        for raw in [
            SCRIPT,
            "<img src=x",
            "\"><img",
            "<script>alert(3)",
            "javascript:",
            "sk-live-",
            "hunter2",
        ] {
            assert!(!html.contains(raw), "{name} leaks {raw:?}: {html}");
        }
    }
    assert!(rendered[0].1.contains("[REDACTED]"));
    assert!(
        rendered[11].1.contains(escaped_breakout),
        "comment text reaches the dialog escaped"
    );
}

#[test]
fn approval_ticket_binds_to_the_displayed_header_and_current_revision() {
    use yoke_core::plan;
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().canonicalize().unwrap();
    let database = project.join("state.db");
    let store = Store::open(&database).unwrap();
    let c = Console {
        home: project.clone(),
        project: project.clone(),
        database,
        token: "token".into(),
        csrf: "csrf".into(),
        host: "127.0.0.1:1".into(),
    };
    let created = plan::create(&store, &project, ".yoke/plans", "bind", "Bind").unwrap();
    let header = |title: &str| {
        format!(
            "+++\ntitle='{title}'\n[[check]]\nname='diff'\nrun='git diff --check'\n+++\n\n# Bind\n"
        )
    };
    std::fs::write(&created.file, header("First")).unwrap();
    let mut actions = actions::Actions::default();
    let apply = |actions: &mut actions::Actions, value: serde_json::Value| {
        actions.apply(&store, &c, serde_json::from_value(value).unwrap())
    };
    let ticket = |prepared: &serde_json::Value| {
        let html = prepared["data"]["confirmation"].as_str().unwrap();
        html.split("data-ticket=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_owned()
    };
    let prepare = serde_json::json!({"action":"prepare-approval","slug":"bind"});
    let prepared = apply(&mut actions, prepare.clone()).unwrap();
    assert!(prepared["data"]["confirmation"]
        .as_str()
        .unwrap()
        .contains("SHA-256"));
    let first = ticket(&prepared);
    // The agent edits the contract after the human opened the confirmation.
    std::fs::write(&created.file, header("Second")).unwrap();
    let approve = |ticket: &str| serde_json::json!({"action":"approve","ticket":ticket});
    let refused = apply(&mut actions, approve(&first)).unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("plan header or check inputs changed after they were displayed")
            && refused.to_string().contains("run yoke plan approve again"),
        "{refused:#}"
    );
    assert_eq!(plan::get(&store, &project, "bind").unwrap().state, "draft");
    // A refused ticket is spent, so the human has to review the new header.
    let spent = apply(&mut actions, approve(&first)).unwrap_err();
    assert!(
        spent
            .to_string()
            .contains("Approval expired or already used"),
        "{spent:#}"
    );
    let second = ticket(&apply(&mut actions, prepare.clone()).unwrap());
    apply(&mut actions, approve(&second)).unwrap();
    let approved = plan::get(&store, &project, "bind").unwrap();
    assert_eq!(approved.state, "approved");
    assert!(approved
        .approved_header
        .as_deref()
        .unwrap()
        .contains("title='Second'"));
    // A ticket prepared before another transition is refused by the revision guard.
    let stale = ticket(&apply(&mut actions, prepare).unwrap());
    plan::cancel(&store, &project, "bind", "superseded").unwrap();
    let concurrent = apply(&mut actions, approve(&stale)).unwrap_err();
    assert!(
        concurrent.to_string().contains("plan changed concurrently"),
        "{concurrent:#}"
    );
    assert_eq!(
        plan::get(&store, &project, "bind").unwrap().state,
        "cancelled"
    );
}

#[test]
fn remote_sessions_skip_the_browser_and_get_a_tunnel_command() {
    let env = |pairs: &'static [(&str, &str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    };
    assert!(headless_ssh(env(&[("SSH_CONNECTION", "1 2 3 4")])));
    assert!(!headless_ssh(env(&[
        ("SSH_CONNECTION", "1 2 3 4"),
        ("DISPLAY", ":0")
    ])));
    assert!(!headless_ssh(env(&[])));
    // A missing opener is a note for the user, never a reason to stop serving.
    let note = open_link("yoke-no-such-opener", "http://127.0.0.1:1/").unwrap();
    assert!(note.contains("yoke-no-such-opener"), "{note}");
    assert!(tunnel_hint(4242).contains("ssh -N -L 4242:127.0.0.1:4242"));
}
