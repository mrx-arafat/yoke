# Core worker w4: integration contract

Status: W4 source/manifests frozen; local integration verified; root owns final Git delivery.
Ownership: CLI main/hook/commands and integration tests; core event/wire/lib; all Cargo manifests and lockfile.
No Git writes, live installation, user settings writes, or binary replacement.

## Current expanded acceptance snapshot

Final post-migration checks: Core CLI 7/7 passed in 1.31s; CLI clippy -D warnings passed exit 0 in 2.56s.
Roadmap CLI 5/5 passed after atomic Stop and standalone review binding; console 6/6 passed after bounded transport replacement.
Migration 006 removes per-page decision counting from Sessions; w1 reports four focused storage tests passing, including backfill, trigger maintenance, retention and all six reverse migrations.
W6 final post-migration native rebuild, package and runtime validation passed at version 0.1.0.
Native binary: 8,415,648 bytes, SHA256 b3e32bfe234d57f10ef46f28b54cae7a67cd5709ed38868cfc13fdbc1555df2e.
Archive: 3,194,268 bytes, SHA256 4abb93a628257df9a2769117ae62e56c5863c953ebab8ebab85dcd0836ad54a0; exact artifact location is recorded at the top of core-worker-w6.md.
W6 verified complete embedded notices and current CSS/JS/SVG, schema 6 runtime, read-only statusline state and exact wrapper stdin.
Thirty native release statusline launches measured median 4.314ms, p95 4.651ms and maximum 4.960ms; all were below 15ms.
The other three native targets remain CI-unrun, and no publishing, tagging or live binary replacement occurred.
Post-migration Sessions retime on the same 100006-decision fixture passed: 18.3ms click-to-main/next-frame, 7.1ms fetch, 6.5ms response; previous 324.2/310.2ms miss is resolved.
All 24 visual/theme/width combinations have zero observed accessibility violations and document overflow; approval/resume and memory actions passed actual browser interaction.
Lighthouse scores remain unproven because its CLI is absent and the sole browser has no attachable debugging port; real day/week soak remains unrun.
Universal first-paint targets and approved-baseline comparisons are not claimed: initial uninstrumented prototype paint was 120ms, later 204ms measurement included accessibility instrumentation.
Final scoped rustfmt and owned whitespace checks passed; current lock passed cargo-deny licenses/sources after tiny_http removal.
Authenticated actual-builder review and digest each succeeded once on disposable fixtures under explicit user authorization; credentials/settings were not copied or changed.
No unresolved high-severity blocker was reported by w3's final read-only migration/transport/auth review; its scope was source review, not extra runtime evidence.
Root is the sole Git writer under the latest authorization; w4 performed no Git writes, live install/uninstall, publishing, user binary replacement, browser operation or subagent spawning.
W4 will stand by without further builds or test expansion unless a concrete delivery finding requires an owned fix.

### Integration evidence journal

Central CLI check/build passed with all roadmap core modules and console prototype/statusline registered.
Actual temporary-home CLI journeys passed: memory mapped export/import/idempotence and no-write dry-run; statusline identical wrapper input/no state creation; plan session binding, human refusal, generic prompt non-resume and real shell proof.
Peer focused evidence: w2 plan 7/7 plus config requirement 1/1; w3 engine 13/13, review 5/5, stop guard 4/4 and core clippy; w1 memory 4/4 and store 3/3.
The single isolated real Codex reviewer attempt accepted flags but returned HTTP 401 without credentials, exit 1 in 18208 ms; w3 recorded fail-closed error evidence, not successful model acceptance.
W5 owns active console prototype browser acceptance and remaining full pages.
W6 focused presets 4/4, usage 3/3 and statusline 3/3 passed; real 30-launch statusline timing median 9.423 ms, p95 11.914 ms and unchanged database bytes are peer-reported.
W4 roadmap_cli now passes all 5 focused real-CLI tests: memory/context, statusline, FIFO rejection, digest scheduling/debounce and plan wait/stop-cap/check workflow.
Actual isolated PTY captured-header/digest approval and typed resume passed through approved/building/waiting/building states.
W3 authenticated actual-builder Codex acceptance subsequently passed, exit 0 with findings=[] and error=null in 8325 ms, under the newly explicit user authorization.
W4 final Core regression passed 7/7 in 1.19 seconds after output-delivery uniqueness and usage ingestion assertions.
New audit follow-up is active: cap-reached invalid transcript initially stalled the plan before returning deny; transcript preflight now precedes guard mutation and digest starts only after decision persistence, with the focused CLI regression GREEN.
W1/w3 are implementing an atomic stop guard/facts/decision API to also roll back guard state on final SQLite write failures; integration remains pending until that source lands.
Standalone CLI review now captures a pre-run fingerprint and uses w2's persisted success result, so changed-tree or fingerprint-error evidence cannot be printed or exited as pass.
Full console build and its 3 focused tests passed; w5 retains browser acceptance and audit fixes.
Installer statusline now has peer-reported 8/8 tests plus real temporary-home dry-run/idempotence/exact restoration proof.
W1 authenticated actual-builder digest succeeded on disposable synthetic facts in 9625 ms: 3 facts, 3 memories, 3 provenance links, persisted readonly reopen, no live settings changes.
Atomic stop integration and standalone-review CLI regression are now GREEN.
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test --locked -q -p yoke-cli --test roadmap_cli --test core_workflow`: Core 7 passed/0 failed in 1.56s; roadmap 5 passed/0 failed in 1.32s.
The combined focused run covers invalid transcript at reached cap preserving building state/transitions/no digest claim, observe leaving stop state untouched, atomic stop behavior, and a real fake-reviewer process editing the tree that persists void and returns CLI failure.
W3 atomic stop suite passed 6/6, including failed fact/decision inserts at both block and stall boundaries, with full transaction and caller-row rollback.
Outstanding: console transport/browser audit completion and final scoped lint/format evidence.
Latest exact trust-reader regression passed centrally: 1 passed, 61 filtered, 0 failed, 0.04 seconds.
W4 scoped rustfmt check and owned diff whitespace check pass.
Latest CLI clippy reached one w5-owned needless borrow in console/actions.rs:83; sent to w5 without modifying peer source, final rerun awaits bounded HTTP transport integration.
W5 fixed that borrow and confirmed all tiny_http source references removed for the std::net replacement.
W4 removed tiny_http from workspace/CLI manifests, regenerated Cargo.lock offline (80 packages) and reran the actual prebuilt cargo-deny license/source gate: exit 0, licenses ok/sources ok, only unused allowlist warnings.
Visible refreshed icon/select/theme has actual desktop/mobile interaction screenshots from w5; remaining console acceptance is bounded transport and plan/memory action journeys, not another redesign.
Bounded std::net transport is integrated: w5 reports shared build exit 0 and console:: 6 passed/0 failed in 0.16s, including w2 socket framing/deadline/SSE limits.
W5 fixed console.rs:218 redundant borrow; final `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo clippy -p yoke-cli --bin yoke --locked -- -D warnings` passed exit 0 in 1.45s.
W4 scoped rustfmt remains clean after atomic Stop and bound-review integration.
Final owned diff whitespace check exited 0.
W4 source/manifests/lock are frozen pending any concrete browser finding; no unimplemented CLI placeholder remains.
Browser acceptance and native packaging remain owned by w5/w6, with real soak periods explicitly unrun.
Latest w5 browser evidence: exact frozen-header approval and human waiting-to-building resume pass; memory pin/share/forget/undo pass.
The 24 page/theme/width sweep found two Usage keyboard-scroll findings and two mobile Trust overflow findings; fixes built successfully and only those four cases are being rerun.
The Sessions page measured 315 ms remains an explicit performance gap, not a claimed sub-100ms pass.
Native release inputs are prepared by w6 but compilation remains gated on the final console source freeze; visible preview was delivered first.
Final browser UI evidence now passes all 24 page/theme/width combinations: initial 20 plus four targeted fixed-case reruns, no observed WCAG violations or horizontal overflow.
Sessions timing confirmed a real 310ms server response; w1 owns read-only EXPLAIN investigation before any change.
Per orchestrator speed direction, w6 has the sole native compile slot on the current source snapshot while w1 investigates without edits; any resulting store fix requires a subsequent incremental native rebuild before final artifact claims.
Stop output and digest eligibility derive the committed decision row, and post-commit digest scheduling errors go to redacted stderr/error.log without retroactively denying an allowed stop.
W2 standalone fingerprint regression passed; trusted file reader now rejects nonregular/symlink/over-1MiB inputs without blocking, with focused GREEN and old-reader revert RED evidence.
README owner should document that new trust-file limit and the now-implemented Claude-only statusline wrapper installation.
Real day/week soak, nonnative release builds and publishing remain unrun.

Latest exact w4 evidence:
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test --locked -q -p yoke-cli --test roadmap_cli`: 5 passed, 0 failed, 1.16 seconds.
FIFO regression first failed in 2.03 seconds with blocked-open assertion, then passed in 0.74 seconds after O_NOFOLLOW/O_NONBLOCK plus regular-file validation.
The CLI now depends directly on existing licensed libc; lockfile updated without a new package version and cargo-deny licenses/sources passed again.
The first PTY harness assertion incorrectly expected ready; inspecting the actual state definition corrected the fixture to approved, after which the complete real-terminal journey passed.
PTY fixture removed known agent markers only from its subprocess environment to exercise interaction mechanics; this remains a best-effort human boundary, not identity authentication.
Digest scheduling uses a fake Codex executable, proves YOKE_OFF=1, read-only/ephemeral flags, persisted digest memory and one spawn across two Stop hooks; no real model was invoked by w4.

## Expanded roadmap contract

The preceding Core evidence remains exact: 7 CLI integration tests, 2 wire tests, clippy, scoped formatting and isolated terminal trust journey passed.
New ownership adds CLI workflow.rs, memory.rs and presets.rs; peer files remain exclusively owned by their authors.
Version stays 0.1.0.

1. In progress: settle workflow/memory/stop/review APIs and database requests with w1-w3; register only licensed dependencies and real modules.
2. Source complete, verification pending: plan/review, memory export/import/map/dry-run and presets commands against implemented APIs.
3. Source complete, verification pending: hook plan context, facts, stop guard, usage and digest integration; preserve silent observe and human-only trust/approve/resume boundaries.
4. Console prototype and statusline registered; isolated CLI journeys in progress and w5 owns browser acceptance before completing console pages.

Proposed CLI boundaries: workflow::run(PlanCommand), workflow::review(...), memory::run(MemoryCommand), presets::run(PresetCommand).
Plan commands use explicit optional --session and --agent metadata, never a flag bypassing human approval.
CLI approve/resume reuse the terminal/known-agent-marker/typed-confirmation boundary.
Hook context is emitted only where public wire contracts allow it and only in enforce mode, preserving observe silence.
Stop evaluation accepts a stable project/session and the verified working-tree fingerprint; database transition ownership belongs to w1/w2/w3 APIs.
Memory export/import accepts --all or current project, explicit repeatable --map source=destination, --dry-run and explicit export overwrite; no imported trust/config/plan approvals or implicit destination paths.
Digest scheduling must claim/debounce through storage and use bounded fixture-verifiable child commands with YOKE_OFF=1.
Request peers publish exact signatures before CLI coupling, with w1 as sole schema/migration owner.
Core review fix implemented: enforced applied rewrites log as rewrite, observe rewrites remain allow, and stdout delivery failures go to error.log without inserting a second decision.
The existing CLI rewrite assertion reproduced allow before the fix (1 failed), then passed (1 passed, 6 filtered).
Unknown mode errors now persist conservative enforce mode with an explicit explanation instead of violating the database mode constraint.
CLI workflow.rs, memory.rs and presets.rs source handlers and command registration are implemented against peer-published APIs.
All ready core modules are exported, including memory/plan/review/stop_guard/preset/usage.
Generic UserPromptSubmit does not authenticate human input, so it does not automatically resume waiting or stalled plans.
Terminal/console human resume remains the only supported boundary; w2/w3 agreed to this public-contract limitation.
Hook usage imports explicit transcript paths at Stop/SessionEnd/PreCompact; unsupported prices remain unknown.
Memory dry-run uses a read-only existing store or an in-memory absent-destination store, never creates destination directories; export reads source state read-only.
Awaiting w1 implementation before focused migration journey.
Shared human boundary helpers were extracted for trust/approve/resume without bypass flags.

Expanded dependency set currently settled: core production tempfile; CLI tiny_http 0.12.0 (MIT OR Apache-2.0) and pulldown-cmark 0.13.4 (MIT, html feature without getopts).
Lockfile refreshed.
`/tmp/yoke-license-prebuilt.ZjZ4Wg/cargo-deny-0.20.2-aarch64-apple-darwin/cargo-deny check licenses sources` after these additions exited 0: licenses ok, sources ok; only unused allowlist warnings.
No temporary checker installation/build was performed by w4.
Disk constraint: peers pause builds; slots use CARGO_INCREMENTAL=0 and CARGO_BUILD_JOBS=2; shared target is preserved by w4.
All w4 temporary test fixtures already self-cleaned, so no identifiable persistent w4 artifact was deleted or space reclamation claimed.
Latest central `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo check -p yoke-cli --quiet --locked` and matching `cargo build -p yoke-cli --bin yoke --quiet --locked` exited 0.
The only remaining build warnings were w5 console prototype home/database fields, handed to w5 as full pages are implemented.
Shared debug binary handed to w5 for temporary-home browser proof.
Disk recovered externally from 273 MiB to 4.6 GiB; w4 claims no cleanup credit.
w2 first focused plan test slot exited 101 before running tests because w1 memory/tests.rs was not yet saved; no placeholder file was added.
Fixture-only Git writes are explicitly authorized in fresh temporary repositories; shared project Git writes remain forbidden.
Focused CLI evidence with CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2:
`cargo test --locked -q -p yoke-cli --test roadmap_cli` passed the initial 2 tests (memory transfer and statusline), 0 failures, 0.79 seconds.
`cargo test --locked -q -p yoke-cli --test roadmap_cli plan_cli_records` passed 1 test, 2 filtered, 0 failures, 0.99 seconds.
The real CLI plan fixture verifies noninteractive approve/resume refusal, recorded session binding, waiting state surviving generic UserPromptSubmit, and actual shell check output with persisted done state.
Approval/resume setup used public core APIs in the fixture, so this test does not claim human PTY approval acceptance.
Memory transfer checks pinned/text/source/timestamp/identity preservation, explicit project remap, duplicate no-op and fresh-home dry-run with an empty directory afterward.
Statusline uses real CLI stdin and cat wrapper, checks exact appended input and no state directory creation.
Remaining w4 checks include final Core regression after hook additions, stop cap/context/digest scheduling journey, presets runtime, and human workflow PTY; w5 console remains prototype pending browser acceptance/full pages.

## Proposed interfaces

- Hook CLI: `yoke run-hook <event> --agent <claude|codex>` with Claude as compatibility default.
- Hook flow: YOKE_OFF check before parsing or state access; trusted effective config; normalized event; evaluation; redacted decision persistence; agent-specific blocking output only in enforce mode.
- Core event and wire types remain compatible with the existing skeleton where feasible; supported Codex events require public-contract verification.
- Trust CLI requires an interactive terminal and explicit human confirmation; no yes/force/noninteractive bypass.
- Config and log commands expose bounded structured output and actionable errors.
- Dependencies and module exports are added only after peer contracts are read and licenses checked.

## Working plan

1. Completed: read required sources and peer contracts, agree module APIs, verify public hook schemas.
2. Completed: implement event/wire and CLI trust/config/log/hook integration.
3. Completed: wire install/update exports and dependency/module requests.
4. Completed: focused module/CLI tests and isolated temporary-home workflow, with evidence below.

## Evidence and open gaps

Peer APIs integrated, including atomic SessionEnd recording and update interval claims.
Public contracts verified at https://code.claude.com/docs/en/hooks and https://learn.chatgpt.com/docs/hooks.
Codex now documents SessionEnd; Notification remains Claude-only.
Claude rewrites omit permissionDecision to preserve ordinary permission evaluation; Codex requires allow with updatedInput and string command for Bash/apply_patch.
AXI guidance applied to bounded tables, version fast paths and help; hook silence and terminal-only explicit trust confirmation override generic output/noninteractive guidance.

## Completed evidence

- `cargo check -p yoke-cli --quiet --locked`: exit 0.
- `cargo test -p yoke-cli --test core_workflow --quiet --locked`: final completed run 7 passed, 0 failed, 1.43 seconds.
- `cargo test -p yoke-core wire::tests --lib --quiet --locked`: 2 passed, 0 failed, 23 filtered.
- `cargo clippy -p yoke-cli --bin yoke --locked -- -D warnings`: final run exit 0 after the error-mode ordering fix.
- Isolated PTY scenario via `uv run --no-project python`: reviewed captured config, typed trust confirmation, inspected persisted enforce config, executed real Stop hook and observed exact block output; exit 0.
- Peer-reported integrated focused checks: install 6/6 and update 6/6; each peer records commands and evidence in its note.
- Scoped rustfmt applied only to w4 files with `skip_children=true` so peer modules were not reformatted; final `rustfmt --check` for all seven w4 Rust files exited 0.
- Final `git diff --check` scoped to w4 owned files exited 0; no Git write commands were used.
- Concrete late regression RED: `cargo test -p yoke-cli --test core_workflow hook_uses_payload_cwd_and_preserves_permission_checks_on_rewrites --quiet --locked` exited 101 with the assertion that payload-cwd enforce mode must deny mismatched events.
  The fixed hook resolves payload cwd and mode before event consistency checks, and marks unresolved payload cwd mode unknown so a failed lookup cannot silently reuse another project's observe mode.
  The final 7-test run above is GREEN.

The CLI tests cover untrusted config, silent observe, enforce deny for both agents, stale snapshot use, redacted filtered log, config origins/freshness, malformed input, inaccessible DB fallback, YOKE_OFF before parsing/state, agent trust refusal and missing bypass flags, payload cwd, rewrite permission behavior, isolated install dry-run/idempotence/uninstall, lifecycle decisions, internal gate errors, advisory events, and a check-only daily child using fake curl with YOKE_OFF and throttling.
The PTY was a disposable test fixture with known agent markers removed solely to exercise the human-terminal branch; this proves interaction mechanics, not identity authentication.

## Remaining concerns and limits

No known local integration blocker remains from w4's focused checks.
The default config view now includes full gate values alongside declaration origins, as well as scalar values, snapshot freshness and disk-read errors.
w1 made SessionEnd closure atomic with decision persistence; the separate CLI end-session call was removed.
w3's final rewrite guard fix was included in the final integration build; w3 owns its RED/GREEN engine evidence.
Tests were added after initial implementation for most integration paths; the concrete error-mode regression above has explicit RED/GREEN evidence.
No full test suite was run.
No live installation, live binary update, publishing, browser, subagent, or Git write occurred.
The manifest version remains 0.1.0 pending the orchestrator's release/version decision.
Automatic checks are bounded child processes, not a full working-day real-agent trial; that acceptance remains unrun.
Mode cannot be known when the state DB cannot be opened, so blocking hooks conservatively deny with stderr and redacted error.log fallback.
If the home/state directory is itself unwritable, stderr explicitly reports that the fallback could not be written.
`cargo deny --version` reports the command is absent; no installation was performed.
Full metadata includes r-efi with `MIT OR Apache-2.0 OR LGPL-2.1-or-later`; MIT/Apache alternatives are allowed, while the repository's simple forbidden-substring scan reports this false positive.
All newly requested direct dependencies were checked with `cargo info`: rusqlite MIT; regex, sha2, tempfile, libc and toml_edit MIT OR Apache-2.0.
Release workflow license gating remains w6's responsibility, and final README/starter/design correction remains the orchestrator's ownership.

## Public sources used

- https://code.claude.com/docs/en/hooks : input fields, blocking output, and permission-preserving updatedInput behavior.
- https://learn.chatgpt.com/docs/hooks : supported events, Codex command input, narrower rewrite output and SessionEnd support.
- https://toonformat.dev/reference/spec : compact CLI table/string output conventions, never hook output.
- crates.io metadata via `cargo info` for requested dependency versions/licenses; no competitor implementation material was used.

## Handoff

CLI exports are wired for all Core commands: init, gates, run-hook, trust, config, log, install, uninstall, update.
YOKE_OFF exits before parsing or state access; normal allow and observe hook stdout is empty.
Internal errors persist as error decisions, deny blocking events in enforce or unknown mode, and use redacted stderr/error.log when storage fails.
Daily SessionStart checks use atomic interval claims, a bounded update --check child, YOKE_OFF=1, no inherited stdin/stdout, and stderr in error.log.
Human trust has no yes/force bypass and requires an interactive terminal, absence of known agent markers, reviewed captured bytes and the exact trust confirmation.
The environment/terminal checks are best-effort friction, not a sandbox or identity authentication.
The orchestrator should now review the combined implementation and sync public docs, without a live install or release claim.
