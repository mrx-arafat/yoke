//! Lexical recall benchmark; see docs/decisions/0001-memory-retrieval.md.
//! Print the table with: cargo test -p yoke-core --test recall_benchmark -- --nocapture

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::Deserialize;
use yoke_core::{memory, store::Store};

#[derive(Deserialize)]
struct Fixture {
    memories: Vec<String>,
    queries: Vec<Query>,
}

#[derive(Clone, Deserialize)]
struct Query {
    query: String,
    class: String,
    relevant: Vec<usize>,
}

#[derive(Debug, Default)]
struct Score {
    recall_at_3: f64,
    recall_at_5: f64,
    mrr: f64,
    empty: usize,
    misses: Vec<String>,
}

fn score(fixture: &Fixture, mut search: impl FnMut(&str) -> Result<Vec<usize>>) -> Result<Score> {
    let mut score = Score::default();
    for query in &fixture.queries {
        let ranked = search(&query.query)?;
        if ranked.is_empty() {
            score.empty += 1;
        }
        let found = |k: usize| {
            query
                .relevant
                .iter()
                .filter(|id| ranked.iter().take(k).any(|hit| hit == *id))
                .count() as f64
                / query.relevant.len() as f64
        };
        score.recall_at_3 += found(3);
        score.recall_at_5 += found(5);
        match ranked.iter().position(|hit| query.relevant.contains(hit)) {
            Some(rank) => score.mrr += 1.0 / (rank + 1) as f64,
            None => score.misses.push(query.query.clone()),
        }
    }
    let count = fixture.queries.len() as f64;
    score.recall_at_3 /= count;
    score.recall_at_5 /= count;
    score.mrr /= count;
    Ok(score)
}

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "do", "does", "for", "from", "how", "i", "in", "is", "of", "the",
    "to", "what", "when", "where", "which", "why",
];

/// A standalone FTS5 table so alternative tokenizers and query forms can be compared.
fn alternative(
    fixture: &Fixture,
    tokenizer: &str,
    any_term: bool,
    stopwords: bool,
) -> Result<Score> {
    let connection = Connection::open_in_memory()?;
    connection.execute_batch(&format!(
        "CREATE VIRTUAL TABLE bench USING fts5(text, tokenize='{tokenizer}')"
    ))?;
    for (id, text) in fixture.memories.iter().enumerate() {
        connection.execute(
            "INSERT INTO bench(rowid,text) VALUES (?1,?2)",
            params![id as i64, text],
        )?;
    }
    score(fixture, |query| {
        let terms = query
            .split_whitespace()
            .filter(|term| !stopwords || !STOPWORDS.contains(&term.to_lowercase().as_str()))
            .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let expression = terms.join(if any_term { " OR " } else { " AND " });
        let mut statement = connection.prepare(
            "SELECT rowid FROM bench WHERE bench MATCH ?1 ORDER BY bm25(bench),rowid DESC LIMIT 10",
        )?;
        let rows = statement.query_map([expression], |row| row.get::<_, i64>(0))?;
        Ok(rows
            .map(|row| row.map(|id| id as usize))
            .collect::<rusqlite::Result<_>>()?)
    })
}

#[test]
fn lexical_recall_benchmark() -> Result<()> {
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/recall-benchmark.json"))?;
    assert!((30..=60).contains(&fixture.memories.len()));
    assert!((15..=25).contains(&fixture.queries.len()));
    let directory = tempfile::tempdir()?;
    let project = directory.path().to_str().unwrap().to_owned();
    let store = Store::open(directory.path().join("state.db"))?;
    let mut ids = Vec::new();
    for (index, text) in fixture.memories.iter().enumerate() {
        ids.push(memory::remember(&store, &project, text, false, index as i64)?.id);
    }
    let shipped = score(&fixture, |query| {
        Ok(memory::recall(&store, &project, query, 10)?
            .iter()
            .filter_map(|row| ids.iter().position(|id| *id == row.id))
            .collect())
    })?;
    let rows = [
        ("shipped memory::recall", shipped),
        (
            "AND unicode61 (recall before migration 010)",
            alternative(&fixture, "unicode61", false, false)?,
        ),
        (
            "OR unicode61",
            alternative(&fixture, "unicode61", true, false)?,
        ),
        (
            "OR porter",
            alternative(&fixture, "porter unicode61", true, false)?,
        ),
        (
            "OR porter stopwords",
            alternative(&fixture, "porter unicode61", true, true)?,
        ),
    ];
    println!("variant | recall@3 | recall@5 | MRR | empty results");
    for (name, row) in &rows {
        println!(
            "{name} | {:.3} | {:.3} | {:.3} | {}/{}",
            row.recall_at_3,
            row.recall_at_5,
            row.mrr,
            row.empty,
            fixture.queries.len()
        );
        println!("  no relevant hit: {:?}", row.misses);
    }
    for class in ["identifier", "keyword", "paraphrase"] {
        let subset = Fixture {
            memories: fixture.memories.clone(),
            queries: fixture
                .queries
                .iter()
                .filter(|query| query.class == class)
                .cloned()
                .collect(),
        };
        let shipped = score(&subset, |query| {
            Ok(memory::recall(&store, &project, query, 10)?
                .iter()
                .filter_map(|row| ids.iter().position(|id| *id == row.id))
                .collect())
        })?;
        println!(
            "{class} ({}) shipped recall@5 {:.3} MRR {:.3}",
            subset.queries.len(),
            shipped.recall_at_5,
            shipped.mrr
        );
    }
    // Shipped text is redacted on write, so it can rank slightly differently from the raw variants.
    assert!(rows[0].1.recall_at_5 >= 0.85, "lexical recall regressed");
    assert!(rows[0].1.mrr >= 0.8, "lexical ranking regressed");
    Ok(())
}
