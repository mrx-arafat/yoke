//! Parent-scoped accounting for Codex rollouts with explicitly attributed child records.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use yoke_core::{store::Store, trust, usage};

fn write_rows(path: &Path, rows: &[Value]) -> Result<()> {
    let mut file = fs::File::create(path)?;
    for row in rows {
        writeln!(file, "{row}")?;
    }
    Ok(())
}

fn parent_meta() -> Value {
    json!({"type":"session_meta","payload":{"id":"parent","session_id":"parent","source":"cli"}})
}

fn child_meta(child: &str, session: &str) -> Value {
    json!({"type":"session_meta","payload":{
        "id":child,"session_id":session,"parent_thread_id":"parent",
        "forked_from_id":"parent",
        "source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent","depth":1}}}
    }})
}

fn attributed(thread: &str, response: &str, input: u64, cached: u64, output: u64) -> Value {
    json!({"type":"token_usage_record","timestamp":"2026-10-17T18:00:00Z","payload":{
        "thread_id":thread,"session_id":"parent","response_id":response,
        "usage":{"input_tokens":input,"cached_input_tokens":cached,"cache_write_input_tokens":0,"output_tokens":output}
    }})
}

fn cumulative(input: u64, cached: u64, output: u64) -> Value {
    json!({"type":"event_msg","timestamp":"2026-10-17T18:00:00Z","payload":{
        "type":"token_count","info":{"total_token_usage":{
            "input_tokens":input,"cached_input_tokens":cached,"output_tokens":output
        }}
    }})
}

fn totals(store: &Store, project: &str) -> Result<(u64, u64, u64)> {
    let rows = usage::summaries(store, Some(project), Some("parent"))?;
    Ok(rows.iter().fold((0, 0, 0), |sum, row| {
        (
            sum.0 + row.input_tokens,
            sum.1 + row.output_tokens,
            sum.2 + row.cache_read_tokens,
        )
    }))
}

#[test]
fn linked_child_usage_counts_once_under_the_parent_without_changing_its_anchor() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("project");
    let outside = temp.path().join("outside");
    fs::create_dir(&project)?;
    fs::create_dir(&outside)?;
    let project = project.canonicalize()?;
    let project_name = project.to_str().context("fixture path must be UTF-8")?;
    let store = Store::open(temp.path().join("state.db"))?;
    let anchor = trust::session_anchor(&store, "codex", "parent", &project)?;
    let parent = temp.path().join("parent.jsonl");
    let parent_rows = [parent_meta(), cumulative(100, 20, 10)];
    write_rows(&parent, &parent_rows)?;
    usage::ingest(&store, "codex", "parent", project_name, &parent)?;
    assert_eq!(totals(&store, project_name)?, (80, 10, 20));

    let unrelated = temp.path().join("unrelated.jsonl");
    write_rows(
        &unrelated,
        &[
            child_meta("unrelated", "another-session"),
            attributed("unrelated", "unrelated-response", 999, 0, 999),
        ],
    )?;
    assert!(
        usage::ingest(&store, "codex", "parent", project_name, &unrelated).is_err(),
        "a parent link must not override a conflicting session identity"
    );
    assert_eq!(totals(&store, project_name)?, (80, 10, 20));

    let child = temp.path().join("child.jsonl");
    let first_response = attributed("child", "child-response-1", 10, 2, 3);
    let mut child_rows = vec![
        child_meta("child", "parent"),
        parent_meta(),
        attributed("parent", "copied-parent-response", 100, 20, 10),
        cumulative(100, 20, 10),
        json!({"type":"turn_context","payload":{"model":"gpt-6.1-sol"}}),
        first_response.clone(),
        first_response,
        cumulative(110, 22, 13),
        attributed("child", "child-response-2", 5, 1, 2),
        cumulative(115, 23, 15),
    ];
    write_rows(&child, &child_rows)?;
    usage::ingest(&store, "codex", "parent", project_name, &child)
        .context("a linked Codex child rollout must be attributed to its parent")?;
    // Parent: 80 uncached / 10 output / 20 cached; child: 8+4 / 3+2 / 2+1.
    assert_eq!(totals(&store, project_name)?, (92, 15, 23));
    usage::ingest(&store, "codex", "parent", project_name, &child)?;
    assert_eq!(totals(&store, project_name)?, (92, 15, 23));

    let sibling = temp.path().join("sibling.jsonl");
    let mut nested_meta = child_meta("sibling", "parent");
    nested_meta["payload"]["parent_thread_id"] = json!("child");
    nested_meta["payload"]["source"]["subagent"]["thread_spawn"]["parent_thread_id"] =
        json!("child");
    nested_meta["payload"]["source"]["subagent"]["thread_spawn"]["depth"] = json!(2);
    write_rows(
        &sibling,
        &[
            nested_meta,
            attributed("child", "copied-child-response", 50, 0, 10),
            attributed("sibling", "sibling-response", 7, 0, 1),
            cumulative(7, 0, 1),
        ],
    )?;
    usage::ingest(&store, "codex", "parent", project_name, &sibling)?;
    assert_eq!(totals(&store, project_name)?, (99, 16, 23));

    child_rows.push(attributed("child", "child-response-3", 4, 0, 1));
    write_rows(&child, &child_rows)?;
    usage::ingest(&store, "codex", "parent", project_name, &child)?;
    assert_eq!(totals(&store, project_name)?, (103, 17, 23));

    // A conflicting repeat must fail atomically, including on incremental reads.
    child_rows.push(attributed("child", "child-response-3", 999, 0, 1));
    write_rows(&child, &child_rows)?;
    assert!(usage::ingest(&store, "codex", "parent", project_name, &child).is_err());
    assert_eq!(totals(&store, project_name)?, (103, 17, 23));

    // A child replacement changes only its own stream, not the parent or sibling.
    write_rows(
        &child,
        &[
            child_meta("child", "parent"),
            attributed("child", "replacement-response", 6, 1, 2),
        ],
    )?;
    usage::ingest(&store, "codex", "parent", project_name, &child)?;
    assert_eq!(totals(&store, project_name)?, (92, 13, 21));
    let agents = usage::totals_by_agent(&store, 0)?;
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].totals.input_tokens, 92);
    assert_eq!(agents[0].totals.output_tokens, 13);

    // Replacing the parent transcript must retain both children's accounting streams.
    let replacement = temp.path().join("parent-replacement.jsonl");
    write_rows(&replacement, &parent_rows)?;
    fs::rename(&replacement, &parent)?;
    usage::ingest(&store, "codex", "parent", project_name, &parent)?;
    assert_eq!(totals(&store, project_name)?, (92, 13, 21));
    assert_eq!(
        trust::session_anchor(&store, "codex", "parent", &outside)?,
        anchor,
        "usage attribution must not move the parent policy anchor"
    );
    assert!(usage::summaries(&store, Some(project_name), None)?
        .iter()
        .all(|row| row.session_id == "parent"));
    Ok(())
}
