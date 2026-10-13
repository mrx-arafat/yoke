# Core worker w3

## Final delivery review

Bounded read-only review found no unresolved high-severity blocker in migration 006 interactions or the current console transport/auth implementation.
Migration 006 backfills session counts and maintains them on decision insert/delete/session move within SQLite transactions; the existing decision-session foreign key prevents orphaned count updates.
Console transport now limits live connections and queued requests, enforces absolute read/write deadlines, rejects duplicate headers and ambiguous body framing, and closes each response connection.
Application routes retain token, exact Host, POST Origin/CSRF/content-type checks; SSE has four-stream, duration, and output bounds.
The prior unbounded trust-preview read and Undo ID-reuse/ticket-consumption issues are corrected in current source.
Limits: source review only, no Cargo, browser, network adversarial tests, new model calls, or production edits; root remains the only Git writer.

## Console read-only security audit

Sent these implemented-code findings to w5 and w4; w5 owns fixes and any core coordination.
No browser, tests, models, or peer-file edits were used.

1. `console/views.rs:167` calls `fs::read` before checking the 1 MiB preview limit, so an oversized trust file is allocated in full and a FIFO can block the synchronous server.
   Open with nonblocking/no-follow flags, validate regular-file metadata, and cap the read itself.
2. `console.rs` reads an authenticated action body with a byte cap but no deadline on the sole request handler.
   A partial body can block other console requests indefinitely; use bounded read time and bounded concurrency rather than detached unbounded workers.
3. `console/actions.rs` removes Undo tickets before restore, while core `restore_snapshot` reinserts the old numeric memory ID.
   Forget the highest-ID memory, create another memory (SQLite reuses the ID), then Undo fails uniqueness and loses the ticket.
   Restore under an available local ID while preserving portable identity and provenance; consume the ticket after successful restoration.

Token comparison, exact Host and POST Origin checks, CSRF header, escaped Markdown without user links/images, approval header digest/revision validation, screenshot type/size checks, and the four-stream SSE cap are present in implemented code.
No concrete authentication bypass was identified in this source pass; this is not a browser or adversarial runtime verification claim.

## Atomic Stop persistence implementation, focused verification passed

W4 fixed the transcript preflight and post-persistence digest ordering with a focused regression, but separate guard and decision/fact transactions still permit partial state on a database write failure.
Approved and implemented `stop_guard::record_stop(store: &Store, project: &Path, max_blocks: u32, row: &mut DecisionRecord, payload_cap: usize, raw: &Value) -> Result<StopDecision>` uses the prepared decision's session/time/mode/base outcome.
Compute the fingerprint before one immediate transaction; recheck active state, apply guard, finalize a cloned decision, then write hook facts and decision in that same transaction.
Publish the modified row only after commit; existing `evaluate` remains compatible.
W1 saved the agreed crate-private transaction-aware `Store::record_decision_in` and `memory::record_hook_facts_in` extractions without schema or existing public API changes.
Only a base `allow` evaluates the guard; existing deny/would_deny/error rows cannot increment counters or stall a plan.
The public wrapper validates event/session, resolved mode, base outcome, gates array, and canonical project identity before persistence.
Two focused regressions cover fact/decision INSERT failures at both block and stall paths, caller-row rollback, successful retry, existing gate denial at cap zero, and observe-mode hypothetical logging without guard writes.
Source and tests are saved and formatted.
In w4's granted slot, `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test --locked -q -p yoke-core stop_guard::tests` passed 6 tests, zero failed/ignored, 56 filtered, 0.09s, exit 0.
Scoped `git diff --check` passed.
Cargo slot released immediately to w4 for final CLI integration checks; no additional model, browser, or broad test execution occurred.

## Final integration audit for root and w4

Read-only source audit covered CLI hook/digest/review integration, core digest parsing/persistence, reviewer execution, and stop sequencing.
No peer source edits, tests, builds, browser, additional model calls, or Git writes were performed for this audit.

1. **P1: Stop state commits before later failure can deny the Stop.**
   `crates/yoke-cli/src/hook.rs:146` invokes the mutating guard before digest scheduling at line 243, usage ingestion at line 264, and decision persistence at line 274.
   With an active plan at the cap and a missing or malformed transcript, the guard commits `stalled` plus its transition/fact, may launch a digest, then transcript ingestion errors and `fail()` denies the Stop.
   The plan now requires human resume even though the stop was never permitted.
   Move fallible transcript/preflight work before guard mutation and do not launch digest until the final hook decision is successfully persisted; preserve fail-closed behavior.
   Suggested focused regression: cap reached plus invalid transcript denies without a stalled transition/fact or digest launch.
2. **P2: Standalone review can attach passing findings to an unreviewed fingerprint or exit successfully after evidence validation fails.**
   `crates/yoke-cli/src/workflow.rs:183` runs review before `plan::record_review`; `crates/yoke-core/src/plan/checks.rs:204` computes only the post-run fingerprint.
   A concurrent source edit lets tree A findings be stored as pass under tree B's fingerprint.
   A post-review fingerprint error is saved as error evidence but `record_review` returns success and the CLI tests the original `report.passed`, allowing exit 0.
   Bind standalone evidence to before/after fingerprints, void changed-tree results, and propagate evidence validation failure to CLI status.
   Mandatory `plan check` already compares before/after fingerprints and is not implicated.

Both defects were sent to w4; standalone evidence details were also sent to w2 for ownership coordination.
These are source-confirmed findings with explicit reproduction scenarios, not claimed runtime reproductions.
The reviewed digest core rejects timeout/nonzero/truncation/malformed JSON and invalid fact references before its transaction, and reviewer execution retains failed evidence.
No additional concrete defect was identified in the owned engine/review/stop modules during this pass.
Available for w5 console security review when that implementation settles.

## Completed follow-up

Added a redacted `plan_transition` fact atomically with the stop guard's stalled transition using the existing session and migration 2 schema.
The JSON detail matches w2: `{plan,from,to,actor,note}` and passes through `redact_payload(detail,4096)` before insertion.
The extended regression proves a rejected fact write rolls back plan state and transition history, then verifies the successful fact's session/project/time/kind and redacted detail.
The prior deliberately unauthenticated run demonstrates failure handling only, not an external authentication blocker.
The user now authorized one sequential existing-auth acceptance attempt; `codex login --help` documented `status`, and `codex login status` returned `Logged in using ChatGPT` with exit 0.
W4 granted the exclusive model slot and coordinated Cargo slot; the explicitly ignored authenticated fixture used the actual builder and 120-second bound successfully.
No credentials are inspected, printed, copied, or modified.
Both slots were released after completion.

## Expanded workflow APIs (implemented, focused fixtures passed)

`engine::execute_bounded(command: &mut std::process::Command, input: &[u8], timeout: std::time::Duration, output_cap: usize) -> std::io::Result<ProcessOutput>`.
`ProcessOutput { stdout: Vec<u8>, stderr: Vec<u8>, exit_code: Option<i32>, success: bool, timed_out: bool, stdout_truncated: bool, stderr_truncated: bool, ms: u64 }`.
The runner configures pipes and a private Unix process group; callers configure executable/args/cwd/env explicitly, including `YOKE_OFF=1` for model children.
Captured bytes are bounded tails; callers requiring complete JSON must reject `stdout_truncated`.
The existing gate evaluation API remains unchanged and uses this runner internally.

`review::Reviewer::{Claude,Codex}` and `Reviewer::select(configured: &str, building_agent: &str) -> Result<Reviewer>` choose the independent reviewer.
`review::run(project: &Path, reviewer: Reviewer) -> Result<ReviewReport>` uses verified CLI flags, a 600-second timeout, and tracked diff plus untracked nonignored file data as review input.
`review::run_with_command(project: &Path, command: &mut Command, timeout: Duration) -> Result<ReviewReport>` supports explicit fixture executable/arguments without modifying process-wide PATH.
`review::command(Reviewer) -> Command` builds the verified headless invocation for review or digest, with stdin prompts, plain final-answer stdout, and `YOKE_OFF=1`.
Claude uses text output, disables tools and MCP, and does not persist sessions.
Codex uses ephemeral read-only execution, disables shell/unified-exec and agent/plugin/app/code-mode/image features, and ignores user config/rules.
`ReviewReport { findings: Vec<Finding>, passed: bool, error: Option<String>, exit_code: Option<i32>, output_tail: String, ms: u64 }`.
`Finding { severity: Severity::{Blocking,Advisory}, file: String, line: u64, summary: String }` uses strict deserialization and validates nonempty fields plus positive line numbers.
Report persistence uses plan-owned check evidence; w2/w4 must store findings and failure evidence with the review check, including standalone review invocations.

`stop_guard::evaluate(store: &Store, project: &Path, session: &str, max_blocks: u32, now: i64, enforce: bool) -> Result<StopDecision>`.
`StopDecision { block: bool, stalled: bool, reason: Option<String> }`.
It computes the real plan fingerprint and reads/writes counters in an immediate transaction; observe mode computes the would-block decision without counters/transitions/state writes.
`stalled` is true only for an actual transition, never for an observe-mode prediction.
Request w1 schema: `stop_guard(session_id,plan_id,blocks,last_fingerprint,last_block_at,last_transition_id,last_check_run_id)` with composite session/plan key and references.
Monotonic transition/check row IDs, not timestamps alone, prove progress even when events occur within one second.
Request w2 exact plan/session/transition/check schema, `fingerprint(project) -> Result<String>`, and human-resume API before coupling SQL.
Request w4 export `review` and `stop_guard`; no new dependency expected.

## Expanded execution plan

1. Agree schema/interfaces and verify reviewer CLI contracts.
2. Expose and test the bounded process runner; implement strict reviewer findings with fixture processes.
3. Implement persistent stop guard, progress resets, cap-to-stalled transition, and observe mode without mutations.
4. Run focused behavioral checks, publish exact evidence, and hand integration to w4.
The orchestrator authorized one sequential real-model reviewer acceptance run with a 120-second maximum; it failed honestly on missing authentication, as recorded below.
Elapsed day/week acceptance remains unrun.

### Expanded implementation progress and corrections

The reusable runner is implemented in `review/process.rs` and exported through `engine` to keep source sizes bounded without changing gate APIs.
It polls nonblocking stdin and both output pipes concurrently, reports byte-tail truncation, bounds per-poll work, and cleans up the process group on normal exits, timeouts, and I/O errors.
Review fixtures cover strict findings, advisory/blocking outcomes, malformed/unknown fields, nonzero exit, timeout, truncated responses, missing executables, and actual Git read discovery.
The stop guard owns parameterized queries against w1 migrations 3/4 and uses an immediate transaction for counters plus the stalled transition.
Its fixtures cover persistence across reopen, observe-mode immutability, same-timestamp history progress, atomic rollback, and fingerprint errors.
Stop hints read `sessions.agent` with the active plan and include the required `--agent claude` or `--agent codex` in `yoke plan check`.
Unknown recorded agents fail closed before counter writes; a focused fixture covers both valid hints and that rejection.
The hint change passed `rustfmt --edition 2021 crates/yoke-core/src/stop_guard.rs` and the focused stop-guard tests below.
No automatic resume follows `UserPromptSubmit`: its public input does not authenticate a human, so w4/w2 require a verified human terminal or authenticated console action.
`require_command` now checks current PATH for a regular executable on every gate evaluation without launching a process; unavailable wrappers skip that rewrite before first-match selection.

Design correction requested by the orchestrator: mandatory review now includes untracked nonignored files, matching the fingerprint's scope instead of silently accepting an untracked-only implementation.
Untracked regular files are bounded UTF-8 text with executable-mode metadata; symlinks are represented by link-target metadata without following them.
Binary tracked diffs, binary/invalid-UTF-8 untracked files, unsupported file types, escaping paths, and total input over 1 MiB produce failed evidence.
One fixture explicitly sends untracked source content and symlink metadata to reviewer stdin and rejects binary/escaping input.

Disk constraint: no release builds or duplicate target directories were created.
All completed test fixture directories were automatically removed; there are no identified owned disposable build artifacts to remove, so reclaimed space is 0 bytes.
Expanded Cargo checks ran only in w4's granted slot with `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2`.

### Expanded verification evidence

All commands below used `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2`.
`cargo test --locked -q -p yoke-core engine::`: 13 passed, zero failed, 44 filtered out, 1.02s.
`cargo test --locked -q -p yoke-core review::tests`: 5 passed, zero failed, 52 filtered out, 0.17s before the opt-in acceptance fixture was added.
After adding that explicitly ignored acceptance fixture, the same filter passed 5 tests, zero failed, 1 ignored, 53 filtered out, 0.26s.
`cargo test --locked -q -p yoke-core stop_guard::tests`: 4 passed, zero failed, 53 filtered out, 0.03s.
`cargo clippy --locked -q -p yoke-core --lib -- -D warnings`: exit 0.
Scoped `git diff --check` passed; no full suite or workspace-wide clippy was run.

One real attempt ran through the actual `review::command(Reviewer::Codex)` builder and bounded runner in the explicitly ignored `isolated_real_codex_acceptance` fixture.
The fixture creates and automatically removes a tiny temporary repository, home, and Codex home; it clears the child environment and supplies only PATH, temporary HOME/CODEX_HOME, and YOKE_OFF=1.
No Git write command, live settings modification, or credential copying was performed.
`cargo test --locked -q -p yoke-core review::tests::isolated_real_codex_acceptance -- --exact --ignored --nocapture`: acceptance failed, 0 passed/1 failed, 18.22s.
Installed Codex v0.162.1 accepted the builder flags and reported read-only sandbox, then exited 1 after HTTP 401 Unauthorized, "Missing bearer or basic authentication in header".
Yoke returned `passed=false`, `error="reviewer failed (exit Some(1))"`, `exit_code=1`, and `ms=18208`, preserving bounded failure evidence.
This proves real CLI launch and fail-closed authentication handling, not successful model review or model output parsing.
That isolated attempt alone did not verify successful model review; the subsequent authenticated result is recorded below.
Real day/week soak acceptance remains unverified.
Cargo and model slots were released to w4 after completion, and w1 was notified to await its own assigned model slot.

### Atomic fact and authenticated acceptance evidence

All Cargo commands used `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2` in w4's explicit slot.
RED: `cargo test --locked -q -p yoke-core stop_guard::tests::counters_persist_observe_is_readonly_and_cap_stalls_atomically -- --exact` failed as expected because no fact insert reached the rejecting trigger (0 passed, 1 failed, 59 filtered).
GREEN: `cargo test --locked -q -p yoke-core stop_guard::tests` passed 4 tests, zero failed, 56 filtered, 0.04s.
`rustfmt --edition 2021 crates/yoke-core/src/stop_guard.rs crates/yoke-core/src/review/tests.rs` and scoped `git diff --check` passed.
`cargo test --locked -q -p yoke-core review::tests::authenticated_real_codex_acceptance -- --exact --ignored --nocapture` passed 1 test, zero failed, 59 filtered, 8.33s.
The real Codex v0.162.1 subprocess returned plain `[]`, exit 0, with Yoke report `passed=true`, `findings=[]`, `error=null`, `ms=8325`.
This used the existing authenticated CLI context only after documented `codex login status` reported ChatGPT login; no credentials were read, printed, copied, or settings modified by the acceptance harness.
The actual command builder used ephemeral read-only execution with tool features disabled and YOKE_OFF=1 against a disposable tiny repository, which was automatically removed.
The successful result validates real CLI launch, authenticated response, strict findings parsing, and result handling for this fixture; it does not establish review quality on arbitrary code or Claude runtime acceptance.

### Additional verified public interfaces

- `codex exec --help`, `codex features list`, and `claude --help` were inspected without launching a model.
- https://learn.chatgpt.com/docs/non-interactive-mode confirms stdin prompts, progress on stderr, and final-answer stdout for ordinary `codex exec`.
- https://code.claude.com/docs/en/headless distinguishes plain text output from agent-specific JSON envelopes.
- https://raw.githubusercontent.com/openai/codex/main/codex-rs/features/src/lib.rs and installed feature listing confirm the disabled feature names.
No new external response schema was invented; Yoke's own findings schema is parsed independently from plain model response text.

## Proposed interfaces and dependencies

Owns only `engine.rs`, `guard.rs`, and this note.
`engine::evaluate(&Config, EventKind, &AgentEvent) -> Outcome` remains available.
Proposed `evaluate_with_context(..., &EvaluationContext) -> Outcome` supplies explicit cwd, project and home paths.
`Outcome` retains `allow`, `ran`, `reasons`, and `blocked()` and adds `updated_input: Option<Value>`, `gates: Vec<GateResult>`, and `error: bool` for internal execution/config errors.
`GateResult` serializes `name`, `result`, `ms`, and `exit_code` for decision logging.
`guard::evaluate(EventKind, &AgentEvent, home: &Path, cwd: &Path) -> Option<String>` inspects actual tool input before gates.
`guard::agent_process_signal() -> Option<String>` provides documented best-effort human-command refusal signals, not authentication.
w4 declared `guard` and added `regex`, `libc`, and test-only `tempfile`.
w2 and w3 agreed `GateAction::{Check,Deny,Rewrite}`, `run: String` with empty default for non-check gates, `timeout: u64` seconds (minimum one), optional regex/replacement fields, and `rewrite_field: String` default `command`.
`EvaluationContext` contains public `cwd`, `project`, and `home` fields, all `PathBuf`.
`GateResult.result` values are `pass`, `fail`, `deny`, `timeout`, `rewrite`, or `error`.
The guard records `name = "self-protection"` and returns before any user gate executes.
Mode conversion remains the caller's responsibility, so observe mode records the same evaluation without emitting a block or rewrite.

## Progress

Read README, compliance, design, and execution ownership contracts.
Implemented full-match tool regex, payload regex, two-pass check/deny-before-rewrite evaluation, command environment, per-gate timing and exit results.
Shell execution uses a separate Unix process group, nonblocking bounded pipe drains, a monotonic deadline, and group cleanup including on successful shell exit.
Added guard checks on actual command and file/patch target inputs, including existing symlink parents and non-existing file leaves.
Both lexical paths under `.yoke` and resolved paths pointing into it are protected, including a protected-directory symlink pointing outward.
The first selected rewrite is never replaced by a later one; an unchanged result emits no updated input and records `pass`, avoiding needless permission bypass.
Post-integration review found that a rewrite could introduce a protected command after the initial guard.
The engine now also guards the proposed rewritten input before emitting it; a denial keeps `updated_input` absent and records the attempted rewrite followed by `self-protection` denial.
Human detection checks publicly documented Claude markers and Codex thread/session markers observed in this actual worker subprocess.
No Git writes, live settings changes, binary replacement, browser, or subagents.

## Verification

RED: `cargo test -p yoke-core engine::tests::tool_regex_matches_alternatives_but_not_substrings -- --exact` failed with 0 passed, 1 failed against the original literal matcher.
First implementation run: `cargo test --locked -p yoke-core engine::tests -- --nocapture` could not compile while peer-owned `trust.rs` and Gate fields had not landed.
GREEN: `cargo test --locked -q -p yoke-core engine::tests` passed 9 tests, 16 filtered out, zero failures (1.00s).
GREEN: `cargo test --locked -q -p yoke-core guard::tests` passed 3 tests, 22 filtered out, zero failures.
The engine tests run real Unix shell commands in temporary directories and verify actual environment values, capped stderr, deny-before-rewrite ordering, failed-check rewrite suppression, first-only rewrites, missing-shell errors, guard precedence, and child PID disappearance after a 1-second timeout.
The outward-symlink case was added to the existing guard test and failed first, then passed after checking lexical as well as resolved targets.
`cargo clippy --locked -q -p yoke-core --lib -- -D warnings` passed with exit 0.
`rustfmt --edition 2021 --check crates/yoke-core/src/engine.rs crates/yoke-core/src/guard.rs` and scoped `git diff --check` passed.
No `SHORTCUT:` markers were found in owned modules.
No full suite was run.
Post-integration RED: `cargo test --locked -q -p yoke-core engine::tests::rewrite_cannot_introduce_a_protected_operation -- --exact` failed on `out.blocked()` for a rewrite from `safe` to `yoke trust`.
Post-integration GREEN: `cargo test --locked -q -p yoke-core engine::tests` passed 10 tests with zero failures after the second guard check.
The combined CLI workflow is w4-owned and not claimed by this lane.
Public README synchronization is orchestrator-owned; this note records the changed behavior for that update.

## Public sources and limitations

- https://code.claude.com/docs/en/env-vars documents `CLAUDE_CODE_CHILD_SESSION=1` for tool/hook children and `CLAUDECODE=1` for a wider set of subprocesses and IDE terminals.
- https://raw.githubusercontent.com/openai/codex/main/codex-rs/protocol/src/shell_environment.rs identifies thread/session markers and shows insertion of `CODEX_THREAD_ID` into child environments.
- This worker's real shell environment contains nonempty `CODEX_THREAD_ID` and `CODEX_SESSION_ID`.
- https://learn.chatgpt.com/docs/hooks is the public Codex input contract verified by w4: Bash carries command text and apply_patch carries patch text in `tool_input.command`.
- https://code.claude.com/docs/en/hooks is the public Claude tool-input contract.
- https://raw.githubusercontent.com/openai/codex/main/codex-rs/apply-patch/src/parser.rs verifies patch file-header syntax only; no parser implementation was copied.

Environment markers can be unset or spoofed and conservative Claude detection can refuse a human shell inside an IDE terminal.
Marker absence and a TTY do not authenticate a human; CLI confirmation remains separately required.
Claude markers were verified against official documentation, not a newly launched Claude runtime; no agent subprocess was launched for this research.
The guard is deliberately not a shell parser or sandbox: obfuscated commands, indirect scripts, alternate tools, or an agent with equivalent filesystem access can bypass it.
Shell command text matching can reject harmless mentions of protected commands or `state.db`.
Unix process-group cleanup covers normal descendants; a deliberately detached descendant can escape the group.
No private agent or competitor internals were accessed.
