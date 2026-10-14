# 0001 Memory retrieval: fix lexical recall, defer semantic retrieval

Status: accepted on 2026-10-14 for issue #18.

## Context

Memory search uses SQLite FTS5 over `memories.text` (`crates/yoke-core/src/memory.rs:83`).
Before this change every query term had to appear in a memory, because the quoted terms were joined with `AND`, and the index used the default `unicode61` tokenizer without stemming.
Session start injects pinned memories, trusted knowledge and recent memories without any query (`crates/yoke-cli/src/hook.rs:228`), so ranking affects `yoke recall`, the console Memory page and `yoke recall --explain`, not the default injection.
People and agents often search in natural language, and the `AND` form returns nothing unless every word, including words such as "how" or "the", appears in one memory.

## Method

The fixture `crates/yoke-core/tests/fixtures/recall-benchmark.json` holds 45 memories written in the style of Yoke's own project notes and 25 queries with hand-labelled relevant memories.
The queries are 6 code identifiers (for example `claim_interval`, `Store::open`), 6 keyword queries and 13 paraphrased or natural-language questions.
The harness `crates/yoke-core/tests/recall_benchmark.rs` measures the shipped `memory::recall` and alternative FTS5 configurations built on a standalone table.
Recall@k is the share of relevant memories found in the top k results, averaged over queries.
MRR is the mean reciprocal rank of the first relevant result.
Run it with `cargo test -p yoke-core --test recall_benchmark -- --nocapture`.

Limitations: one author wrote both the memories and the labels, the fixture is small and English only, and it contains no pinned memories.
The numbers show direction and size of the gap, not production retrieval quality.
The shipped path redacts text on write, which changes three fixture memories that mention tokens or passwords, so its ranking differs slightly from the raw variants.

## Results

| Variant | Recall@3 | Recall@5 | MRR | Queries with no results |
|---------|----------|----------|-----|-------------------------|
| `AND`, unicode61 (recall before this change) | 0.380 | 0.380 | 0.380 | 15 of 25 |
| `OR` with BM25, unicode61 | 0.780 | 0.800 | 0.827 | 2 of 25 |
| `OR` with BM25, porter unicode61 | 0.880 | 0.880 | 0.860 | 1 of 25 |
| `OR` with BM25, porter unicode61, English stopwords removed | 0.880 | 0.880 | 0.840 | 1 of 25 |
| Shipped after this change (`OR`, BM25, porter unicode61, redacted text) | 0.880 | 0.880 | 0.840 | 1 of 25 |

By query class, recall@5 before the change was 1.000 for identifiers, 0.417 for keywords and 0.077 for paraphrases.
After the change it is 1.000 for identifiers, 1.000 for keywords and 0.769 for paraphrases, with MRR 1.000, 0.917 and 0.731.
The three queries that still miss are "format rust code" (the memory says `rustfmt`), "database schema upgrade" (the memory says migrations) and "deleting old history" (the memory says retention prunes old decisions).
All three are vocabulary mismatches that no tokenizer setting fixes.

## Decision

1. Match any quoted query term and rank by BM25 (`fts_query`, `crates/yoke-core/src/memory.rs:106`); quoting still keeps user text from injecting FTS5 operators.
2. Rebuild `memories_fts` with `tokenize='porter unicode61'` in migration 010 so plural and verb forms match; the index is rebuilt from `memories`, and the down migration restores `unicode61` without losing rows.
3. Order search results by BM25 before pinned status, because with any-term matching a pinned memory may share only one common word with the query; a query-less recall still lists pinned memories first, and session start injection is unchanged.
4. Do not remove stopwords; BM25 already discounts common words, and the stopword variant did not improve recall.
5. Do not add semantic retrieval now: after the lexical fix, 3 of 25 queries (12 percent) miss and recall@5 is 0.88, which does not justify a model dependency yet.
6. Keep regression floors in the benchmark test: recall@5 at least 0.85 and MRR at least 0.80.

The porter stemmer is designed for English, so non-English memories gain nothing from it; they still match on exact tokens as before.

## What semantic retrieval would take

- A local embedding model, because Yoke is local only and must not send memory text to a network service.
- A new inference dependency and a model file, each needing a size, license and supply-chain review; no candidate is evaluated here, so their cost is unknown.
- A table of vectors keyed by memory id with the model identity, filled on write and re-embedded when the model changes; that work fits the durable `jobs` table rather than a hook.
- No embedding inside a hook's latency budget.
- Hybrid ranking, for example reciprocal rank fusion of BM25 and vector similarity, re-measured on a larger fixture that includes multilingual and pinned memories before it replaces lexical ranking.
- Trigger to revisit: `yoke recall --explain` reports or a larger fixture showing vocabulary-mismatch misses above roughly 20 percent of queries.

## Explainability and bounded injection

`memory::context_items` (`crates/yoke-core/src/memory.rs:292`) returns the exact selection that `memory::context` injects, with the reason (`pinned`, `knowledge`, `match` or `recent`), memory id, source, BM25 rank and score, matched query terms, digest fact provenance and truncation.
Because the injected text is built from the same items, an explanation cannot disagree with the injection, and the total stays within `memory.inject_tokens` times 4 characters including separators.
`yoke recall [query] --explain` prints that selection in the terminal; the explanation itself is never added to agent context.

## Sources

- SQLite FTS5 documentation, sections 4.3.1 (unicode61 token characters), 4.3.3 (porter tokenizer, designed for English), 4.4.3 (external content tables), 6.12 (the `rebuild` command) and 5.1.1 (BM25: better matches are numerically lower): https://www.sqlite.org/fts5.html
