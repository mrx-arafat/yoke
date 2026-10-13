# Core worker w1: store and redaction

## Sessions performance follow-up: migration 006

The existing Sessions count used the covering `decisions_session` index but scanned all decisions for every returned session.
On w5's actual synthetic fixture, read-only system SQLite counted 100006 rows in 296 ms cold, then 10 ms and 7 ms on the same connection.
This supported the measured browser server delay without assuming rendering was responsible.
Migration 006 adds `sessions.decision_count`, backfills exact counts, and maintains them with insert/delete/session reassignment triggers in the writer transaction.
`Store::sessions` reads that field; its public return type and behavior are unchanged.
The paired rollback drops the three triggers and derived column and restores user_version 5, preserving decisions.
Earlier upward migrations remain unchanged.

Focused RED command: `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test -p yoke-core store::tests::session_counts_backfill_and_follow_atomic_decision_changes --lib --quiet -- --nocapture` exited 101 with missing decision_count; the original query took 16.922 ms on that new synthetic fixture.
GREEN command: `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test -p yoke-core store::tests --lib --quiet -- --nocapture` exited 0 (4 passed, 59 filtered, 0.78 s).
The 100005-row fixture verifies upgrade backfill, exact project counts, insertion, retention deletion, session reassignment, and rollback of all three mutations.
The existing disposable rollback fixture now reverses all six migrations successfully.
A separate unfiltered focused rerun passed and measured the first Store::sessions call at 82.448 ms, including process-cold redaction initialization; these synthetic timings do not establish a speedup ratio.
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build -p yoke-cli --quiet --locked` exited 0.
The rebuilt debug CLI was handed to w5 for actual browser timing; w6 must rebuild/repackage native artifacts after this source change.
Cargo slot released; no user state or w5 fixture writes were performed by w1.

## Atomic Stop helpers and collision-safe undo follow-up

Saved `pub(crate) Store::record_decision_in(tx: &rusqlite::Transaction<'_>, row: &DecisionRecord, payload_cap: usize) -> anyhow::Result<()>` and `pub(crate) memory::record_hook_facts_in(tx: &rusqlite::Transaction<'_>, row: &DecisionRecord, raw: &serde_json::Value) -> anyhow::Result<usize>`.
The existing public wrappers open their own transaction; the helpers neither begin nor commit a transaction and retain validation/redaction/session-end behavior.
This permits w3's Stop guard, transition, facts, and decision to commit or roll back together without a schema change.
Full Stop failure-injection integration remains w3-owned.

`restore_snapshot` retains its `Result<()>` signature and stable portable identity, but now allocates a new numeric ID when another note has reused the old one.
The memory, fact links, and imported identity mapping restore in one immediate transaction; the replacement note is unchanged.
Consumers refresh the list/identity after undo rather than assuming the old numeric ID remains authoritative.
The focused imported-note regression also verifies repeated archive import remains idempotent.

RED: `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test -p yoke-core memory::tests::portable_archive_maps_projects_is_idempotent_and_dry_run_does_not_write --lib --quiet` exited 101 with `UNIQUE constraint failed: memories.id` (0 passed, 1 failed, 59 filtered).
GREEN: `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test -p yoke-core memory::tests --lib --quiet` exited 0 (4 passed, 58 filtered, 0.23s).
GREEN: `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test -p yoke-core store::tests --lib --quiet` exited 0 (3 passed, 59 filtered, 0.04s).
Owned Rust files were formatted with rustfmt; Cargo slot released to w4 and source readiness sent to w3/w5.

## Expanded Memory contract (implemented, focused checks passed)

Central store exposes `pub(crate) fn connection(&self) -> &rusqlite::Connection` for trusted in-crate modules and `pub(crate) fn transaction<T>(&self, f: impl FnOnce(&rusqlite::Transaction<'_>) -> anyhow::Result<T>) -> anyhow::Result<T>` using an immediate transaction with rollback on error.
Only w1 adds numbered migrations; peer modules use parameterized SQL against agreed schemas and redact free-text before writes.
Migration 2 adds memory/facts/FTS/provenance/import identities, 3 adds peer-requested plans/transitions/checks/artifacts and integer session plan references, 4 adds stop-guard/usage, and 5 adds nullable verified usage cost and a session read index.
Previously applied upward migrations were not edited.
Module exports and production tempfile are integrated by w4; remaining dependencies already existed.
Callable free functions all take `&Store` first and return `anyhow::Result`: `remember(store, project: &str, text: &str, pinned: bool, now: i64) -> Memory`, `recall(store, project: &str, query: &str, limit: usize) -> Vec<Memory>`, `pin(store, id: i64, pinned: bool) -> ()`, `forget(store, id: i64) -> bool`, `share(store, id: i64) -> PathBuf`.
`Memory` serializes fields `id: i64, identity: String, ts: i64, project: String, source: String, text: String, pinned: bool, shared: bool`.
`record_fact(store, session: &str, project: &str, kind: &str, detail: &str, now: i64) -> i64` records redacted facts.
`context(store, project: &str, query: &str, trusted_knowledge: &str, token_budget: usize) -> String` prioritizes pinned notes, supplied trusted knowledge, then recall within the four-characters-per-token budget.
`export_archive(store, path: &Path, project: Option<&str>, overwrite: bool) -> ExportReport` uses a consistent snapshot and atomic no-clobber file creation by default; None explicitly means all projects.
`import_archive(store, path: &Path, mappings: &BTreeMap<String,String>, dry_run: bool) -> ImportReport` validates version, bounds, identities, references, and explicit absolute project mappings before one transaction.
Reports serialize project mappings and addition/duplicate/conflict counts; conflicts are actionable errors with no writes.
Portable stable identities and a separate local-id mapping preserve import idempotence without importing trust, plans, approvals, settings, or shared status.
`get(store, id: i64) -> Option<Memory>`, `forget_with_snapshot(store, id: i64) -> Option<MemorySnapshot>`, and `restore_snapshot(store, &MemorySnapshot)` support console undo with note, fact references and imported-identity mapping restored atomically.
`MemorySnapshot { memory: Memory, fact_ids: Vec<i64>, imported: bool }`; w5 owns the 30-second server-side undo window.
`record_hook_facts(store, &DecisionRecord, raw: &serde_json::Value) -> usize` handles denial, explicit observed Edit/Write/MultiEdit/apply_patch, and numeric nonzero tool_response.exit_code only.
Absent exit status is unknown, not inferred success; the public agents promise a JSON response value, not a universal exit-code field.
`digest_due(store, session: &str, project: &str, now: i64, min_interval_secs: i64) -> bool` is a read predicate.
`digest_with_command(store, session: &str, project: &str, transcript_tail: &str, now: i64, command: &mut Command, timeout: Duration) -> usize` uses w3's bounded executor and sets YOKE_OFF=1 itself.
The digest's own strict output is `{memories:[{text,fact_ids}]}`; references must belong to the selected pending batch, and insertion plus consumed-fact flags plus last-digest time commit together.
Facts are limited to 100 and 128 KiB of serialized input, transcript data is redacted and capped at 32 KiB, output is capped at 256 KiB, and timeout must be positive and at most 600 seconds.
Overlapping digest results fail the atomic pending-fact check instead of duplicating memories.
Fixture model commands verify failure and rollback paths; the separately authorized authenticated synthetic Codex acceptance below also passed.

### Read APIs for console and statusline

`Store::decision_page(&DecisionFilter, before_id: Option<i64>) -> Vec<DecisionRow { id: i64, record: DecisionRecord }>` uses descending insertion IDs and bounded cursor pagination.
`Store::sessions(project: Option<&str>, limit: usize) -> Vec<SessionSummary>` returns id/agent/project/started_at/ended_at/active_plan/decision_count, with active_plan nullable integer and count i64.
`Store::revision() -> i64` is PRAGMA data_version and must be compared on the same persistent connection to observe other writers.
`Store::status_snapshot(session: Option<&str>, project: Option<&str>, now: i64) -> StatusSnapshot` executes one query with indexed session/project predicates.
Its fields are mode/plan_slug/plan_state/latest_version nullable strings and recent_deny bool; recent denial means deny or would_deny within the preceding hour.
`Store::open_read_only(path)` uses SQLite READONLY plus URI readonly_shm=1, a 5 ms busy timeout, no migration and no creation, and never immutable=1.
Normal writable opens set SQLITE_FCNTL_PERSIST_WAL so subsequent read-only processes can use existing WAL/shm files without creating or modifying them.
Older databases missing required sidecars return unavailable until a normal writable open; the read path does not repair state.

### Portable format and limits

Version 1 archives contain explicit projects, portable memories/facts, and fact-memory identity links.
Limits are 16 MiB serialized, 20,000 combined records, 20,000 relationships, 1,000 projects, and 64 KiB per note/fact text.
Export preflights aggregate bytes/counts in the same snapshot and uses a bounded JSON writer, then same-directory atomic no-clobber persistence unless overwrite is explicit.
An oversized single project is currently unsupported; there is no record-range export flag.
Import rejects symlinks/nonregular files and uses O_NOFOLLOW/O_NONBLOCK plus post-open metadata validation to avoid FIFO blocking and symlink races.
Dry-run uses only a deferred read transaction; w4 uses read-only existing state or an in-memory database when the destination is absent.
Missing/extra project mappings fail, repeated identities preserve local IDs, conflicts never replace local rows, and FTS insert triggers index imported records in the same transaction.
Shared state travels only as shared_reference metadata and imports as locally unshared; import never writes project files, trust, plans, approvals, executable config or installation state.

### Expanded focused verification

On the w4-granted slot, `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test -p yoke-core memory::tests --lib --quiet` exited 0: 4 passed, 0 failed, 55 filtered out, 0.30 seconds.
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test -p yoke-core store::tests --lib --quiet` exited 0: 3 passed, 0 failed, 56 filtered out, 0.06 seconds.
These run real temporary SQLite files and fixture shell processes, including FTS/pinning/context/share, apply_patch facts, absent exit-code handling, malformed digest output, timeout, transactional failure, provenance-preserving undo, mapping/idempotence/conflict/invalid archive/rollback, regular-file/size guards, and byte-for-byte unchanged existing-state dry-run.
w4 separately reports `cargo test --locked -q -p yoke-cli --test roadmap_cli` passing memory transfer and statusline: pinned/text/source/timestamp/identity preserved across different project roots and temporary homes, repeated import no-op, fresh-home dry-run leaves the home empty.
That real CLI evidence is owned by w4 and is recorded in core-worker-w4.md, not re-run by w1.
The first expanded tests were added after source implementation during the coordinated build freeze; no feature-wide RED-first claim is made.

### Rollback maintenance

Each numbered upward migration has a paired `.down.sql` in the same store directory.
Runtime remains forward-only; there is no CLI or production rollback API.
On an explicitly authorized disposable backup only, apply down scripts in descending current user_version order, each inside BEGIN IMMEDIATE / COMMIT; each script restores user_version itself.
Never apply a lower rollback while a higher migration remains installed.
Rollback removes the feature tables/columns and their data; it is not a backup substitute.
The focused fixture reverses all six migrations, checks each user_version and preserved Core decision rows until rollback 1, then upgrades the empty fixture again.
No rollback was run on existing user state.

### Resource and acceptance boundaries

Reclaimed exactly the approximately 26 MiB of completed `/tmp/yoke-w1-tests.beCg8t`, `.gtgDze`, `.rLDTZJ`, and `.zGc2CY` executables created by this worker's earlier harness.
Shared debug target and user files were not removed.
Later builds ran only on w4's explicit slot with incremental compilation disabled and two jobs; no duplicate targets or release builds were created.
The bounded archive-size regression uses one automatically cleaned approximately 17 MiB fixture.
No browser, subagent, Git write, live agent settings, release publishing or installed binary replacement occurred.
Real day/week soak acceptance remains outstanding.
One authenticated Codex digest acceptance passed for synthetic facts; this does not establish general summary quality or real Claude runtime acceptance.

### Final authenticated digest acceptance

The user explicitly authorized one authenticated attempt with a 120-second bound; w4 granted the sole model slot after w3 released it.
`codex login status` reported `Logged in using ChatGPT`, exit 0, without reading, printing or copying credentials.
The installed `codex exec --help` confirms `--skip-git-repo-check`, allowing a disposable synthetic non-Git project without Git writes.
A disposable Rust harness linked existing shared workspace rlibs with rustc, called the actual `review::command(Reviewer::Codex)` builder plus that verified flag, then `memory::digest_with_command(..., Duration::from_secs(120))`.
No permanent test, abstraction, dependency or target tree was added.
Exact child arguments: `codex exec --ephemeral --sandbox read-only --color never --ignore-user-config --ignore-rules --disable shell_tool --disable unified_exec --disable multi_agent --disable plugins --disable apps --disable code_mode --disable code_mode_host --disable image_generation - --skip-git-repo-check`.
The core API sets `YOKE_OFF=1`; existing authentication was inherited without modifying HOME, CODEX_HOME, credentials, or live settings.
The input contained three synthetic Atlas project decisions only: UTC timestamps, bounded jittered retries, and newline-delimited JSON queue records.
One attempt completed with harness exit 0 in 9,625 ms: 3 memories, 3 supplied facts, and 3 persisted provenance links.
Every returned memory had a fact link, every link referenced a supplied fact identity, all supplied facts became digested, and reopening the database read-only retained all three memories.
The real child therefore exited successfully, its strict output parsed, and transactional persistence succeeded.
No model subprocess remained after the bounded API returned; w4 was notified that the model slot was released.
The disposable project/database/archive fixtures cleaned themselves automatically; the separately linked harness files were removed after recording this evidence.

### Final migration limitations for root

Archives support explicit absolute Unix project-root mapping only; no inferred mapping, Windows migration, partial-record export, or automatic conflict merge is implemented.
The limits remain 16 MiB, 20,000 records and relationships, 1,000 projects and 64 KiB text per record; an oversized single project cannot currently be exported.
Import preserves portable identities, source/timestamp/pinned/digest data and fact relationships while allocating destination-local IDs through an explicit identity map.
Source session IDs remain metadata on facts; import does not recreate source sessions, usage data or decisions.
Shared flags travel as references and are not activated locally; no project file, trust/config, plan approval, installation state or executable command transfers.
Conflicting content, metadata, project mappings or provenance fail atomically instead of replacing local records.
Dry-run reports conflicts without writes; ordinary import returns an error on conflicts.
Import/export plaintext may still contain unnamed or encoded secrets outside the documented pattern redactor, so transfer remains user-controlled.
Readonly access requires usable WAL/shm sidecars; normal writable Store opens preserve them, while an older sidecar-less database may need a normal writable open before export/statusline can read it.
Rollback scripts restore earlier schemas in descending order but delete data belonging to removed features; they were tested only on disposable fixtures, and runtime remains forward-only.
Successful synthetic digest and migration CLI evidence do not satisfy full-day/week soak criteria.

### Additional primary sources

https://www.sqlite.org/fts5.html supplies FTS5 external-content trigger maintenance and BM25 query behavior.
https://docs.rs/tempfile/latest/tempfile/struct.NamedTempFile.html supplies atomic persist/persist_noclobber semantics.
https://www.sqlite.org/wal.html and https://www.sqlite.org/c3ref/c_fcntl_begin_atomic_write.html explain read-only WAL constraints and SQLITE_FCNTL_PERSIST_WAL.
The bundled public libsqlite3-sys 0.30.1 sqlite3.c unixOpenSharedMemory code confirms readonly_shm=1 opens the shared-memory file O_RDONLY without O_CREAT.
https://learn.chatgpt.com/docs/hooks is w4's verified source for conditional tool_response handling and canonical apply_patch payloads.

## Public API and dependencies

Proposed modules: `yoke_core::store` and `yoke_core::redact`.
Request w4 add module declarations and dependencies `rusqlite` with bundled SQLite, `regex`, and test dependency `tempfile`; all require permissive-license verification.
`Store::open(path: impl AsRef<Path>) -> anyhow::Result<Store>` opens/migrates the database, WAL and 2-second busy timeout.
All methods use `&self` and return `anyhow::Result`.
`TrustRow { scope, root, file, sha256, content: String, trusted_at: i64 }`; `put_trust(&TrustRow)`, `put_trust_batch(&[TrustRow])`, `get_trust(scope: &str, root: &str, file: &str) -> Option<TrustRow>`, `list_trust(scope: &str, root: &str) -> Vec<TrustRow>`.
Batch writes are atomic; list/get use database snapshots without requiring disk files to still exist.
Trust snapshots intentionally remain exact, because redacting executable config changes the approved program.
`DecisionRecord { ts: i64, session_id, agent, project, event, mode, outcome: String, tool, reason: Option<String>, gates: serde_json::Value, payload: String }`.
`record_decision(&DecisionRecord, payload_cap: usize)` redacts all free-text log fields, reasons and gate output before writing, and creates the session on its first event atomically.
For normalized `event == "session_end"`, the same transaction sets `sessions.ended_at` to the decision timestamp.
Hook callers must remove their separate `end_session` call after recording that event.
`DecisionFilter { project, session, outcome, tool: Option<String>, limit: usize }` defaults to 100 rows; `decisions(&DecisionFilter) -> Vec<DecisionRecord>` returns newest first.
`prune_decisions(now: i64, retention_days: u32) -> usize`; timestamps use Unix seconds and retention deletes strictly older rows.
`end_session(id: &str, ended_at: i64)` marks a session ended.
`get_kv(key: &str) -> Option<String>`, `set_kv(key: &str, value: &str)` store internal update metadata unchanged.
`claim_interval(key: &str, now: i64, interval_secs: i64) -> bool` atomically claims a periodic task using an immediate transaction.
Its interval must be positive, timestamps use Unix seconds, malformed stored timestamps error, and backward clock movement suppresses the next claim until the stored deadline.
`redact::redact(input: &str) -> String`, `redact::redact_payload(input: &str, cap: usize) -> String` mask secrets before a UTF-8-safe byte cap.

## Progress

Read README, compliance policy, design, and execution ownership contract.
Implemented only `store.rs`, `store/001-core.sql`, `store/tests.rs`, and `redact.rs`.
The database parent directory must already exist; caller owns location and directory creation.
Numbered migration 1 creates only trust, sessions, decisions, and kv, with decision indexes, foreign-key integrity, and constrained modes/outcomes.
Opening a future schema version fails explicitly.
Decision insertion and initial session creation share one transaction.
Decision queries are bounded to 10,000 rows, default 100, ordered by timestamp then insertion id descending.
All input values use bound SQL parameters.
Redaction runs on payload, reason, gate JSON values and keys, project, session, agent, event, and tool.
Mode/outcome accept only schema-constrained literals; trust and internal kv metadata remain exact.
Regexes are compiled once and reused.
No external agent schema is needed by this lane.

## Focused evidence

Initial `cargo test -p yoke-core redact::tests --lib --quiet` ran zero tests before w4 declared the module; this was not treated as verification.
Subsequent Cargo test builds were blocked by concurrent incomplete peer modules (most recently missing `trust.rs`).
An isolated `rustc --edition 2021 --test --crate-name yoke_w1_lane` harness imports the owned source modules by absolute path and links the workspace-built anyhow/rusqlite/serde/serde_json/regex/tempfile libraries from `target/debug/deps`.
The harness ran four focused tests: redaction shapes, Unicode caps, real SQLite persistence/filter/retention, and trust/kv/migrations.
Initial isolated run: 4 passed, 0 failed.
Adding Basic authorization coverage reproduced a leak (`Authorization: [REDACTED] hidden-basic`), then an authorization-scheme rule was added before generic key masking.
Final isolated harness after fixes: 4 passed, 0 failed, including four concurrent interval contenders with exactly one successful claim.
After w2 saved trust.rs, `cargo test -p yoke-core redact::tests --lib --quiet`: 2 passed, 0 failed, 23 filtered out, exit 0.
`cargo test -p yoke-core store::tests --lib --quiet`: 2 passed, 0 failed, 23 filtered out, exit 0.
`rustfmt --check --edition 2021 crates/yoke-core/src/redact.rs crates/yoke-core/src/store.rs crates/yoke-core/src/store/tests.rs`: exit 0.
The SQLite tests execute the real store against temporary on-disk databases, including close/reopen and WAL verification.
No full suite was run.

### SessionEnd atomicity follow-up

The existing persistence test now records a SessionEnd and checks its timestamp plus exactly one added decision.
RED: `cargo test -p yoke-core store::tests::persists_redacted_decisions_filters_and_retention --lib --quiet` failed with a NULL ended_at before implementation (0 passed, 1 failed).
GREEN: `cargo test -p yoke-core store::tests --lib --quiet` passed (2 passed, 0 failed, 23 filtered out, exit 0).
A temporary SQLite trigger forces the session timestamp update to fail and verifies that neither an extra decision nor a changed ended_at survives.
`rustfmt --check --edition 2021 crates/yoke-core/src/store.rs crates/yoke-core/src/store/tests.rs` exited 0.
Only owned storage files and this note changed; w4 removes the redundant hook call.

## Public sources and licenses

https://docs.rs/rusqlite/latest/rusqlite/struct.Connection.html supplies connection, transaction, and parameter binding API documentation.
https://www.sqlite.org/pragma.html documents journal mode, busy timeout, foreign keys, and user_version.
https://docs.rs/regex/latest/regex/ documents the regex engine and replacement syntax.
The installed rusqlite 0.32.1 source confirms `Transaction::new_unchecked` for an immediate transaction through `&Connection`.
`cargo metadata --format-version 1 --locked` reports rusqlite 0.32.1 MIT, libsqlite3-sys 0.30.1 MIT, regex 1.13.1 MIT OR Apache-2.0, and tempfile 3.27.0 MIT OR Apache-2.0.
Full dependency policy checking and shared Cargo ownership remain with w4.

## Limits and integration

No live user state was touched, no Git writes occurred, and no browser/subagents were used.
Pattern-based redaction cannot identify arbitrary unnamed secrets or encoded/obfuscated secrets.
Trust snapshots deliberately contain exact human-approved executable content and must not be treated as redacted logs.
The error-log fallback must call `redact` in w4's hook integration because this lane does not own that file sink.
Sessions and trust/kv rows are retained; the configured retention applies to decisions only.
No full-working-day observe trial or live installation was attempted.
