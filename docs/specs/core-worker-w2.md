# Core worker w2: config and trust

## Workflow 0.3 contract (implemented and focused-tested locally)

Follow-up complete: write `plan_transition` and `check_result` facts within the same existing transaction as state/evidence writes using migration 2's agreed schema.
No new public API or migration is requested.
Fact session metadata comes from an existing session whose active_plan matches; creation and other unbound operations use the empty string, never a fabricated session ID.
Details are redacted before insertion; failed fact writes must roll back the associated state/evidence writes.
Transition detail is JSON with plan/from/to/actor/note; creation uses from empty string and omits note.
Check detail is JSON with plan/check/result/exit_code/blocking/output.
The start transaction binds the real session before recording the transition and fact.

Atomic-fact RED evidence: with `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2`, each exact test below exited 101 before the implementation, with 58 filtered out.
`cargo test --locked -q -p yoke-core plan::tests::real_checks_drive_state_and_preserve_advisory_failure_evidence -- --exact` failed on zero transition facts versus three expected.
`cargo test --locked -q -p yoke-core plan::tests::concurrent_state_change_preserves_void_check_evidence_without_overwriting_state -- --exact` failed because the fact-insert failure trigger was never invoked.
After the scoped storage change, the same two commands each exited 0: 1 passed, 0 failed, 59 filtered out; elapsed test times 0.14s and 0.06s.
A peer added another test between RED and GREEN, accounting for the filter count change.
The tests exercise real session attribution, empty metadata for unbound creation, redaction of a question's password, and transaction rollback when a fact insert is rejected.
Only the two affected existing tests were run for this follow-up; the Cargo slot was immediately released for w3.

Owns `plan.rs` and `plan/` in addition to config/trust.
w1 owns migration SQL; plan/storage.rs owns parameterized plan queries using the agreed `Store::connection`/`Store::transaction` APIs.
w4 owns module exports and CLI; w5 owns authenticated browser confirmation.
All public plan APIs return `anyhow::Result` and use explicit project paths; session IDs are supplied by callers, never inferred here.

- `plan::create(store: &Store, project: &Path, plans_dir: &str, slug: &str, title: &str) -> Result<PlanRecord>`.
- `plan::list(store: &Store, project: &Path) -> Result<Vec<PlanRecord>>`.
- `plan::get(store: &Store, project: &Path, slug: &str) -> Result<PlanRecord>`.
- `plan::detail(store: &Store, project: &Path, slug: &str) -> Result<PlanDetail>`.
- `plan::prepare_approval(store: &Store, project: &Path, slug: &str) -> Result<PreparedApproval>` captures validated exact header plus SHA-256 digest for a human review surface.
- `plan::approve(store: &Store, prepared: &PreparedApproval) -> Result<PlanRecord>` verifies disk header still equals captured header and optimistic database revision before freezing it.
- `plan::start(store: &Store, project: &Path, slug: &str, session: &str) -> Result<PlanRecord>` binds existing session, rejects missing session and conflicting active work.
- `plan::ask(store: &Store, project: &Path, slug: &str, question: &str) -> Result<PlanRecord>`.
- `plan::resume(store: &Store, project: &Path, slug: &str) -> Result<PlanRecord>` transitions waiting/stalled to building; caller must establish a human action.
- `plan::active(store: &Store, project: &Path, session: &str) -> Result<Option<PlanRecord>>` is the hook/console lookup.
- `plan::check(store: &Store, project: &Path, slug: &str, reviewer: Option<crate::review::Reviewer>) -> Result<CheckSummary>` executes frozen commands using w3's bounded executor; `review=true` without reviewer produces failed evidence, never done.
- `plan::fingerprint(project: &Path) -> Result<String>` hashes HEAD, binary diff and sorted untracked nonignored names/content.
- `plan::record_review_with_fingerprint(store: &Store, project: &Path, slug: &str, report: &crate::review::ReviewReport, before: &str) -> Result<bool>` saves standalone review evidence bound to the fingerprint captured before invoking the reviewer and returns the persisted success verdict.
  A changed tree produces `void`; a failed post-review fingerprint produces `error`; neither returns success.
  w4 owns capturing the before fingerprint and using the returned verdict in the CLI.
- `plan::record_review(store: &Store, project: &Path, slug: &str, report: &crate::review::ReviewReport) -> Result<()>` remains a compatibility evidence recorder without changing plan state, but cannot record successful proof without a captured before fingerprint.
- `PlanRecord { id: i64, project: String, slug: String, file: String, state: String, approved_header: Option<String>, revision: i64, created_at: i64, updated_at: i64 }`, serializable.
- `PlanDetail { plan: PlanRecord, content: String, transitions: Vec<TransitionRecord>, checks: Vec<CheckRun> }`, serializable.
- `PreparedApproval { plan: PlanRecord, header: String, digest: String }`, serializable; approval compares captured revision and header against current state/disk.
- `CheckSummary { plan: PlanRecord, passed: bool, fingerprint_before: String, fingerprint_after: String, checks: Vec<CheckRun> }`, serializable.
- `CheckRun { id: i64, plan_id: i64, ts: i64, name: String, command: String, exit_code: Option<i32>, output_tail: String, fingerprint: String, blocking: bool, ms: u64, result: String, findings: serde_json::Value, artifacts: Vec<String> }`.
- `TransitionRecord { id: i64, plan_id: i64, ts: i64, from_state: String, to_state: String, actor: String, note: Option<String> }`.
- Human confirmation is external to these primitives; approval must use the exact `PreparedApproval` displayed to the human, never silently prepare a new one after confirmation.

### Requested migration 3 schema

`plans(id INTEGER PRIMARY KEY, project TEXT NOT NULL, slug TEXT NOT NULL, file TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('draft','approved','building','checking','done','waiting','stalled')), approved_header TEXT, revision INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, UNIQUE(project,slug))`.
`transitions(id INTEGER PRIMARY KEY, plan_id INTEGER NOT NULL REFERENCES plans(id) ON DELETE CASCADE, ts INTEGER NOT NULL, from_state TEXT NOT NULL, to_state TEXT NOT NULL, actor TEXT NOT NULL CHECK(actor IN ('human','agent','yoke')), note TEXT)` plus `(plan_id,id)` index.
`check_runs(id INTEGER PRIMARY KEY, plan_id INTEGER NOT NULL REFERENCES plans(id) ON DELETE CASCADE, ts INTEGER NOT NULL, name TEXT NOT NULL, command TEXT NOT NULL, exit_code INTEGER, output_tail TEXT NOT NULL, fingerprint TEXT NOT NULL, blocking INTEGER NOT NULL CHECK(blocking IN (0,1)), ms INTEGER NOT NULL, result TEXT NOT NULL, findings TEXT NOT NULL DEFAULT '[]')` plus `(plan_id,id)` index.
Migration 3 adds nullable INTEGER `sessions.active_plan` and retains the original unused TEXT placeholder as `core_active_plan_legacy`; start updates the integer foreign key transactionally with state/transition.
w3 adds stop_guard requirements directly with w1, including last_transition_id and last_check_run_id.
Plan IDs remain integers throughout.

### Build coordination

Source work only until w4 grants a build slot because disk space is constrained.
Use `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2` in the assigned slot.
No disposable build artifacts owned by w2 remain to reclaim; tempdir test fixtures from completed checks were automatically removed.

### Workflow implementation progress and limits

Source implementation saved in `plan.rs` and `plan/{files,storage,checks,tests}.rs`.
Each source file remains below 800 lines.
The seven focused workflow tests use real Git repositories and SQLite databases inside automatically cleaned temporary directories, with real bounded shell commands.
Only fixture repositories receive Git init/add/commit commands, explicitly authorized in this expanded assignment; the shared checkout receives no Git writes.
First granted-slot attempt: `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test --locked -q -p yoke-core plan::tests` exited 101 before running tests because peer-owned `memory/tests.rs` was not yet present (`memory.rs:265`).
w1 was notified and the build slot was returned immediately to w4; no peer files or placeholder stubs were created.
After w1 saved the module and w4 regranted the slot, `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test --locked -q -p yoke-core plan::tests` exited 0: 7 passed, 0 failed, 49 filtered out, 1.33 seconds.
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test --locked -q -p yoke-core config::tests::rejects_invalid_regex_and_action_combinations_before_trust -- --exact` exited 0: 1 passed, 0 failed, 56 filtered out.
The shared source gained another peer test between commands; filter counts differ accordingly.
The build slot was released immediately after these two focused commands.
`rustfmt --edition 2021 --check crates/yoke-core/src/config.rs crates/yoke-core/src/trust.rs crates/yoke-core/src/plan.rs` and owned-path `git diff --check` passed.
There are no `SHORTCUT:` markers in the owned modules.
Workflow tests were written before the first permitted build rather than verified RED, because the shared build slot and peer test modules were unavailable during source implementation.
The runtime preserves documented `[[check]] run='...'` TOML spelling through serde while using `command` internally.
Plan headers require at least one blocking check or `review=true`; advisory-only headers cannot produce vacuous completion.
`Gate.require_command` is now an optional rewrite-only executable basename validated before trust; w3 owns its PATH availability test.
Screenshot artifacts are explicitly declared with `[[check]] artifacts=['output/image.png']`, must resolve inside the project, and are limited to PNG/JPEG/WebP filenames and 10 MiB regular files.
Recorded artifact paths are canonical and do not grant permission to arbitrary file reads; console must revalidate containment, extension and size when serving.
Generated artifacts must live in Git-ignored output directories to avoid invalidating the complete working-tree fingerprint.
Artifact declarations never exclude source files from fingerprints.
Fingerprint Git output is bounded to 16 MiB per command and errors on truncation rather than accepting incomplete evidence.
Repositories need a valid HEAD commit; an unborn repository fails explicitly.
Untracked symlinks contribute their link target bytes, never followed file contents; nonregular untracked entries fail explicitly.
Generic UserPromptSubmit is not proof of human origin and does not automatically resume plans.
Core approval/resume primitives require caller-established human confirmation; w4 and w5 own those authenticated interaction boundaries.
Actual model reviews, screenshots from a running product, and real day/week soak acceptance are not claimed by these fixture tests.
The check finalizer compares the database revision atomically; if the stop guard or another actor changes state while commands run, completed evidence is saved as `void` and the newer state is preserved.
Standalone review errors are persisted even when the repository cannot be fingerprinted; these rows use an empty fingerprint and an explicit error, never a pass.
Fingerprint comparison occurs before and after execution, as specified; it is not continuous filesystem monitoring.
An interrupted check process can leave `checking` state until the stop guard stalls it and a human resumes; no automatic recovery claims are made.
Combined CLI acceptance is w4-owned; w4 reported its temporary-home CLI checks separately and this worker does not count those as its own test runs.
Public README/design updates remain orchestrator-owned; changed workflow behavior and configuration requirements are documented here for synchronization.

## Proposed public APIs and dependencies

- Owns only `config.rs`, `trust.rs`, and this note.
- Request w4 add `pub mod trust`, `regex`, `sha2`, and test-only `tempfile` dependencies.
- Config exposes `Config`, `Mode`, `GateAction`, `Gate`, validated `Config::parse`, partial-layer `Config::merge`, `LoadedConfig` with effective config, origins, project root and snapshot statuses.
- Proposed gate fields: existing fields retained, `run: String` default empty for non-check actions, `action: GateAction`, `timeout: u64` seconds, optional `match_pattern`, `rewrite_pattern`, `rewrite_to`, `rewrite_field: String` default command.
- Trust exposes preparation of validated exact-byte snapshots separately from persistence after human confirmation.
- Trusted loading uses w1 Store trust records only; disk bytes are read only for discovery and staleness, never as executable config.
- Awaiting w1 store signatures and w3 gate agreement before coupling implementations.
- Human-only detection belongs to w3/w4, based on verified runtime signals, not guessed environment variables.

## Agreed integration signatures

- `config::load_trusted(store: &Store, cwd: &Path, home: &Path) -> Result<LoadedConfig>`.
- `LoadedConfig { config: Config, origins: BTreeMap<String, String>, project_root: PathBuf, snapshots: Vec<SnapshotStatus>, knowledge: Option<String> }` is serializable.
- `SnapshotStatus { scope: String, file: PathBuf, trusted: bool, stale: bool, disk_error: Option<String> }` reports missing/unreadable/changed disk files without changing the executable snapshot.
- `trust::prepare_project(store: &Store, cwd: &Path) -> Result<PreparedTrust>` finds nearest project config or previous snapshot root.
- `trust::prepare_global(store: &Store, home: &Path) -> Result<PreparedTrust>` reads `home/.yoke/config.toml`.
- `PreparedTrust { scope: String, root: PathBuf, snapshots: Vec<PendingSnapshot> }`; each pending snapshot exposes `file: PathBuf`, `sha256: String`, `content: String`, `previous: Option<String>` for confirmation/diff rendering.
- `PreparedTrust::commit(&self, store: &Store) -> Result<()>` validates the candidates again and atomically saves captured bytes through `put_trust_batch`.
- Commit is a persistence primitive, not human authentication; w4 must check w3's verified agent-process signal, interactive terminal and human confirmation before calling it.
- `Mode::{Observe, Enforce}` supports `as_str()` and serde snake-case.
- Root/file storage keys are absolute canonical-root paths encoded as UTF-8; invalid UTF-8 paths fail explicitly.
- Config validation accepts timeout values from 1 second upward; rewrites require PreToolUse plus both pattern and replacement.

## Progress

- Read README, compliance, design and Core execution ownership.
- RED evidence: `cargo test -p yoke-core config::tests::rejects_invalid_regex_and_action_combinations_before_trust -- --exact` exited 101, 1 failed because the skeleton accepted `match_tool='['`.
- Implemented effective defaults, explicit-field layer merging, stable gate replacement order, origins, and validated check/deny/rewrite models.
- Implemented two-phase trust preparation/commit, exact-byte SHA-256, transactional config/knowledge persistence, and trusted-only loading through w1 Store APIs.
- Disk modifications, invalid TOML, unreadable files, and deletions do not replace stored snapshots.
- Canonical directory identity resolves symlink aliases and finds trusted ancestor roots even after disk config deletion.
- A nearer untrusted project config establishes its own boundary and does not inherit a parent project's trusted gates.
- Read w1/w3/w4 contracts and coordinated agreement through explicitly named Herdr agents.
- Read w3's recorded public/observed environment-signal evidence; no competing human-detection implementation was added here.

## Focused verification

- `cargo test -p yoke-core config::tests -- --nocapture`: exit 0, 3 passed, 22 filtered out.
- `cargo test -p yoke-core trust::tests`: exit 0, 4 passed, 21 filtered out.
- Trust tests execute real SQLite persistence and temporary filesystem workflows, including changed/deleted configs, knowledge retention, canonical aliases and no-persistence preparation failures.
- `cargo clippy -p yoke-core --lib -- -D warnings`: exit 0.
- `rustfmt --edition 2021 crates/yoke-core/src/config.rs crates/yoke-core/src/trust.rs`: exit 0.
- No full suite, live user settings, live binary changes, Git writes, browser, publishing, or subagents.

## Deliberate boundaries and integration limitations

- w4 owns human confirmation and refusal inside detected agent processes before calling `PreparedTrust::commit`.
- Commit stores the captured content shown for approval, even if the disk changes during confirmation; subsequent loads immediately mark that snapshot stale.
- Deleted knowledge retains the previous trusted snapshot; there is no revocation command in this lane.
- Re-trusting a deleted config fails; it does not silently clear trust.
- Snapshots must be UTF-8; config must parse and validate, and knowledge must contain no NUL bytes.
- Trust root/path keys use canonical-root absolute paths; moving a project gives it a new identity requiring trust again.
- Origins use each scalar setting path and `gate.<name>` for each entire gate declaration, whose fields are replaced together.
- Public `Config::parse` is the layering entry point; generic serde deserialization does not populate explicit-field metadata.
- The compatibility `config::load(cwd)` now creates the state directory before opening SQLite, including on a fresh home.
- No real CLI human-confirmation acceptance is claimed by this worker; w4 owns isolated CLI integration checks.

## Verification and sources

- Implementation inputs: this repository's design and existing independent skeleton.
- No external schemas used.
- No external agent schemas were invented or consumed by config/trust.

## Fresh-home follow-up

At w4's request, added state-directory creation inside `config::load(cwd)` with contextual error propagation.
The regression test re-executes itself with an isolated temporary HOME to avoid changing environment variables in the parallel test process.
It checks that the public loader creates `.yoke/state.db`, defaults to observe mode, and ignores an untrusted project config requesting enforce mode.
`cargo test -p yoke-core config::tests::compatibility_load_initializes_fresh_home_without_trusting_disk_config -- --exact` first exited 101 with SQLite error 14 (missing parent directory).
The same command after the fix exited 0: 1 passed, 25 filtered out.
`rustfmt --edition 2021 crates/yoke-core/src/config.rs` exited 0.
Only `config.rs` and this handoff note changed for the follow-up; public documentation already describes the safe default, so no public doc correction is needed.

## Standalone review fingerprint follow-up

Read-only integration audit identified that the standalone CLI recorded a post-only fingerprint and used the original reviewer verdict even when evidence persistence recorded a fingerprint error.
Mandatory plan checks already compare before/after fingerprints and were unaffected.
Added and exported `record_review_with_fingerprint` with the exact signature above; the caller must capture `plan::fingerprint` before invoking the model and gate success on the returned boolean.
Changed trees persist `void` against the original fingerprint; failed post fingerprints persist `error`; both return false after successful persistence.
The compatibility recorder delegates with a missing-before sentinel and cannot persist a passing proof.
Extended the existing `mid_run_edits_and_missing_review_never_mark_done` regression for unchanged passing evidence, changed-tree void evidence retaining the before digest, compatibility refusal, and a post-fingerprint error overriding a passing model report.
`rustfmt --edition 2021 crates/yoke-core/src/plan/checks.rs crates/yoke-core/src/plan/tests.rs crates/yoke-core/src/plan.rs` exited 0.
`git diff --check -- crates/yoke-core/src/plan.rs crates/yoke-core/src/plan/checks.rs crates/yoke-core/src/plan/tests.rs docs/specs/core-worker-w2.md` exited 0.
With w4's slot, `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test --locked -q -p yoke-core plan::tests::mid_run_edits_and_missing_review_never_mark_done -- --exact` exited 0: 1 passed, 59 filtered out, 0.78s.
No pre-implementation RED run is claimed for this API extension because source was requested while builds were gated.
w4 owns the CLI caller and focused CLI regression; no peer files, models, browser, or live settings were used.
W4 confirms the CLI captures before the reviewer and uses the persisted boolean for both exit status and printed `report.passed`.
W5 confirms console only reads saved evidence, with no standalone reviewer caller to migrate.

## Trust descriptor-read follow-up

Confirmed metadata-then-reopen could open a swapped FIFO and had no read size bound.
With w4 approval, `read_regular_file` now opens a Unix descriptor using `O_NONBLOCK | O_NOFOLLOW`, verifies descriptor metadata is a regular file of at most 1 MiB, and bounds the read to 1 MiB plus one overflow byte.
Non-Unix platforms reject this operation explicitly; the current supported release targets are Unix.
Rejected disk files retain the trusted database snapshot and expose staleness and disk errors.
Extended the existing preparation/retrust regression for oversized files, symlinks, and a temporary FIFO with no writer.
The initial assertion compared a noncanonical macOS fixture path; corrected it to the canonical snapshot path before final behavioral verification.
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test --locked -q -p yoke-core trust::tests::preparation_and_failed_retrust_leave_old_snapshot_untouched -- --exact` passed with the corrected regression and descriptor fix: 1 passed, 59 filtered out, 0.02s.
A subsequent revert-check using only the old reader failed at `read_regular_file(&file).is_err()` on the oversized file, proving the corrected regression detects the old behavior.
Restored the descriptor fix immediately; its final rerun was compile-blocked by concurrent integration references to pending `memory::record_hook_facts_in` and `Store::record_decision_in` APIs in `stop_guard.rs`.
Released the Cargo slot to w4 and requested the exact test rerun after w1 lands these APIs.
Rustfmt completed successfully; w4 owns adding the regular non-symlink, 1 MiB trust-file limit to README.
W4 subsequently reported the final exact trust regression green: 1 passed, 61 filtered out, 0.04s, exit 0 after the helpers landed.

## Original bounded console transport handoff

New exclusive ownership is `crates/yoke-cli/src/console/http.rs` and its inline tests only; w5 owns console and events integration, w4 owns manifest removal of tiny_http.
Agreed APIs: `Server::bind_loopback() -> io::Result<Server>`, `local_addr() -> io::Result<SocketAddr>`, `recv() -> io::Result<Request>`.
Request accessors are `method() -> &str`, `url() -> &str`, `header(&str) -> Option<&str>` (ASCII case-insensitive), and `body() -> &[u8]`.
`Request::respond(self, status: u16, headers: Vec<(String,String)>, bytes: &[u8]) -> io::Result<()>` closes the connection.
`Request::into_stream(self, status: u16, headers: Vec<(String,String)>) -> io::Result<EventStream>` yields a `Write` implementation for authenticated SSE; w5 retains the four-stream admission limit.
Transport binds IPv4 loopback with port zero and caps all in-flight sockets at 16 using lifetime permits spanning parsing, queueing, responses and SSE.
Per-connection parser workers avoid blocking ordinary route dispatch on an incomplete request.
Headers cap at 16 KiB, request bodies at 64 KiB, normal response bodies at 16 MiB, and event-stream writes at 1 MiB over an absolute 60-second lifetime.
Request reading has one absolute 3-second deadline shared by headers and body; normal writes share one absolute 3-second deadline, with SSE frames also capped to 3 seconds within their lifetime.
The strict local subset accepts HTTP/1.1 GET/POST origin-form requests only and rejects transfer encoding, Expect, duplicate headers (including Content-Length and authentication headers), malformed framing, and overflows.
Connection, Content-Length and Transfer-Encoding response headers belong to the transport; callers must not supply them.
Implementation was written originally from the agreed contract and public HTTP framing reference [RFC 9112](https://www.rfc-editor.org/rfc/rfc9112.html#section-6.3), without copying third-party implementation code.
Three focused inline tests cover framing/caps, real socket trickle and blocked-write deadlines, and SSE plus incomplete-client coexistence with ordinary routes, connection closure, stream caps and permit bounds.
Rustfmt and scoped diff whitespace checks exited 0; central Cargo test execution is delegated to w5's sole build slot and is pending at this handoff.
No browser, model, dependency, manifest or peer source edits were performed by w2 for this transport.
