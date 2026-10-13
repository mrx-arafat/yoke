use super::*;

#[test]
fn every_route_requires_token_host_and_same_origin_mutations() {
    let c = Console {
        home: PathBuf::new(),
        project: PathBuf::new(),
        database: PathBuf::new(),
        token: "secret".into(),
        csrf: "csrf".into(),
        host: "127.0.0.1:1234".into(),
    };
    let mut headers = BTreeMap::from([("host".into(), c.host.clone())]);
    let mut params = BTreeMap::new();
    assert!(!authorize(&c, "GET", &headers, &params));
    params.insert("token".into(), "secret".into());
    assert!(authorize(&c, "GET", &headers, &params));
    assert!(!authorize(&c, "POST", &headers, &params));
    headers.insert("origin".into(), "http://evil.invalid".into());
    headers.insert("x-yoke-csrf".into(), "csrf".into());
    headers.insert("content-type".into(), "application/json".into());
    assert!(!authorize(&c, "POST", &headers, &params));
    headers.insert("origin".into(), format!("http://{}", c.host));
    assert!(authorize(&c, "POST", &headers, &params));
    headers.insert("host".into(), "evil.invalid".into());
    assert!(!authorize(&c, "GET", &headers, &params));
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
