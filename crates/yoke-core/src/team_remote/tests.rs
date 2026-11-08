use super::*;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn roots(base: &Path) -> Roots {
    let home = base.join("home");
    Roots {
        project: base.join("project"),
        claude_dir: home.join(".claude"),
        claude_json: home.join(".claude.json"),
        claude_managed: base.join("managed"),
        codex_home: home.join(".codex"),
        codex_system: base.join("codex-system"),
        home,
    }
}

/// Give the clone an identity so commits work whatever the machine's Git config says.
fn identify(state: &Path) {
    let dir = clone_dir(state);
    for (key, value) in [
        ("user.name", "Test"),
        ("user.email", "t@example.com"),
        ("commit.gpgsign", "false"),
    ] {
        gitcmd::run_ok(&dir, &["config", key, value]).unwrap();
    }
}

#[test]
fn urls_with_credentials_options_or_odd_transports_are_refused() {
    for good in [
        "https://example.com/team/ext.git",
        "ssh://git@example.com/t.git",
        "git@example.com:team/ext.git",
        "/srv/ext.git",
    ] {
        assert!(check_url(good).is_ok(), "{good}");
    }
    for bad in [
        "",
        "-oProxyCommand=x",
        "ext::sh -c id",
        "https://user:token@example.com/x.git",
        "https://exa mple.com/x",
        "ftp://example.com/x",
        "just-a-word",
    ] {
        assert!(check_url(bad).is_err(), "{bad:?} must be refused");
    }
}

#[test]
fn two_people_share_extensions_through_a_remote_without_overwriting_silently() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let bare = base.join("team.git");
    fs::create_dir_all(&bare).unwrap();
    gitcmd::run_ok(&bare, &["init", "--bare", "--quiet"]).unwrap();
    let url = bare.display().to_string();

    // Ana shares a command and a skill with a script into the still-empty remote.
    let (ana, ana_state) = (roots(&base.join("ana")), base.join("ana/home/.yoke"));
    write(&ana.claude_dir.join("commands/ship.md"), "ship v1\n");
    write(
        &ana.claude_dir.join("skills/review/SKILL.md"),
        "---\nname: review\n---\nreview v1\n",
    );
    write(
        &ana.claude_dir.join("skills/review/scripts/run.sh"),
        "echo hi\n",
    );
    let synced = connect(&ana_state, &url, None).unwrap();
    assert!(synced.empty);
    identify(&ana_state);
    let share = |roots: &Roots, relative: &str| {
        shared_from(&files::find(roots, &roots.claude_dir.join(relative)).unwrap()).unwrap()
    };
    let items = [
        share(&ana, "commands/ship.md"),
        share(&ana, "skills/review/SKILL.md"),
    ];
    pull(&ana_state).unwrap();
    let plan = plan_push(&ana_state, &items).unwrap();
    let paths: Vec<_> = plan.changes.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "commands/ship.md",
            "skills/review/SKILL.md",
            "skills/review/scripts/run.sh"
        ]
    );
    assert!(plan.changes.iter().all(|c| c.kind == ChangeKind::Add));
    assert!(
        push(
            &ana_state,
            &items,
            &SharePlan {
                changes: Vec::new(),
                ..plan.clone()
            }
        )
        .is_err(),
        "a plan nobody reviewed is not pushed"
    );
    push(&ana_state, &items, &plan).unwrap();
    assert!(
        plan_push(&ana_state, &items).is_err(),
        "pushing the same thing twice has nothing to send"
    );

    // Ben pulls and sees both; his own differing command is replaced only through the trash.
    let (ben, ben_state) = (roots(&base.join("ben")), base.join("ben/home/.yoke"));
    write(&ben.claude_dir.join("commands/ship.md"), "ben's ship\n");
    connect(&ben_state, &url, None).unwrap();
    let listing = list(&ben_state).unwrap();
    let ids: Vec<_> = listing.items.iter().map(Remote::id).collect();
    assert_eq!(ids, ["claude/skill/review", "claude/command/ship"]);
    let ship = listing.items.iter().find(|r| r.name == "ship").unwrap();
    let local = ben.claude_dir.join("commands/ship.md");
    let compared = compare(ship, Some(&local)).unwrap();
    assert_eq!(
        (
            compared[0].state,
            compared[0].local.as_deref(),
            compared[0].remote.as_deref()
        ),
        (FileState::Differs, Some("ben's ship\n"), Some("ship v1\n"))
    );

    let plan = plan_install(&ben, ship, Scope::User).unwrap();
    assert!(plan.replaces);
    let trash = apply_install(&plan, &ben_state, 1700000000)
        .unwrap()
        .unwrap();
    assert_eq!(fs::read_to_string(&local).unwrap(), "ship v1\n");
    assert_eq!(
        fs::read_to_string(trash.join("ship.md")).unwrap(),
        "ben's ship\n"
    );

    let skill = listing.items.iter().find(|r| r.name == "review").unwrap();
    assert_eq!(skill.files, ["SKILL.md", "scripts/run.sh"]);
    let plan = plan_install(&ben, skill, Scope::Project).unwrap();
    assert!(!plan.replaces);
    apply_install(&plan, &ben_state, 1700000001).unwrap();
    assert!(ben
        .project
        .join(".claude/skills/review/scripts/run.sh")
        .is_file());
    assert_eq!(
        compare(skill, Some(&plan.to))
            .unwrap()
            .iter()
            .filter(|c| c.state == FileState::Same)
            .count(),
        2
    );

    // Disconnecting forgets the address and deletes only the clone.
    disconnect(&ben_state).unwrap();
    assert!(load(&ben_state).unwrap().is_none() && !clone_dir(&ben_state).exists());
    assert!(local.is_file(), "installed extensions stay");
}
