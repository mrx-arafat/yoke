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
    let result = apply(
        &mut actions,
        serde_json::json!({"action":"forget","id":m.id}),
    )
    .unwrap();
    assert!(memory::get(&store, m.id).unwrap().is_none());
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
