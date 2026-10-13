# Core milestone execution

Scope: implement the 0.2 Core milestone from `2026-10-13-yoke-design.md`.
The user authorized orchestration across six existing Codex workers on 2026-10-13.
The user subsequently requested completion of the remaining roadmap with all six workers assigned.
Core integration remains the immediate dependency; later lanes can publish contracts and build isolated modules without destabilizing it.
Memory import/export and cross-environment migration are explicitly included in 0.4.
This is an execution tracker, not a claim that 0.2 acceptance has passed.

## Shared constraints

All workers share `/Users/easinarafat/AI/yoke` and the current branch.
The latest user request authorizes the orchestrator to commit and push this version on the current branch after checks pass.
Workers must not write Git state.
Branch changes, live installation, binary replacement, tagging, and release publication remain outside this authorization.
Preserve concurrent edits and the pre-existing untracked `.codegraph/` directory.
Follow `COMPLIANCE.md`; use Yoke's design and public agent documentation, never competitor implementation material.
Run only focused tests, with real CLI scenarios in isolated temporary homes where appropriate.
w5 owns the sole browser lane for the expanded console milestone.
External agent schemas in the draft are hypotheses until checked against authoritative current documentation or installed public interfaces.
The statusline command and reversible installer integration are implemented; live installation is not part of delivery.

## Ownership

| Worker | Herdr agent / pane | Responsibility | Exclusive writable paths |
|---|---|---|---|
| w1 | yoke-w1 / wZ:p2 | SQLite, decision records, retention, redaction | `crates/yoke-core/src/store.rs`, `crates/yoke-core/src/store/`, `crates/yoke-core/src/redact.rs` |
| w2 | yoke-w2 / wZ:p3 | Config defaults/layering/origins, trusted snapshots | `crates/yoke-core/src/config.rs`, `crates/yoke-core/src/trust.rs` |
| w3 | yoke-w3 / wZ:p4 | Gate actions, matching, bounded execution, self-protection | `crates/yoke-core/src/engine.rs`, `crates/yoke-core/src/guard.rs` |
| w4 | yoke-w4 / wZ:p5 | CLI integration, event normalization, wire output, end-to-end Core checks | `crates/yoke-cli/src/main.rs`, `crates/yoke-cli/src/hook.rs`, `crates/yoke-cli/src/commands.rs`, `crates/yoke-cli/tests/`, `crates/yoke-core/src/event.rs`, `crates/yoke-core/src/wire.rs`, `crates/yoke-core/src/lib.rs`, all Cargo manifests and generated lockfile |
| w5 | yoke-w5 / wZ:p6 | Reversible Claude/Codex install and uninstall | `crates/yoke-cli/src/install.rs`, `crates/yoke-cli/src/install/` |
| w6 | yoke-w6 / wZ:p7 | Verified self-update and release packaging | `crates/yoke-cli/src/update.rs`, `crates/yoke-cli/src/update/`, `.github/workflows/`, `scripts/release/` |

The orchestrator owns this tracker, README synchronization, starter configuration, cross-worker arbitration, and final integration review.
Workers may write only their own `docs/specs/core-worker-wN.md` handoff in addition to the paths above.
Each handoff starts with proposed public interfaces and dependencies, then records implementation state, focused test evidence, source URLs, and open issues.
Worker 4 coordinates shared manifests and module declarations after reading those interfaces.
Workers must not edit a peer's files to resolve an integration error; communicate the required change instead.

## Execution order

1. Publish contracts and dependency requests before implementation; w1/w2/w3 settle storage, config, and outcome interfaces with w4.
2. Implement independent modules with focused behavioral tests; w5/w6 verify external contracts first.
3. Worker 4 integrates the modules and runs isolated real CLI scenarios: trust, observe/enforce hook decisions, decision log, install/uninstall restoration, and error handling.
4. Orchestrator reviews combined changes, resolves reported gaps through the owning worker, and synchronizes documentation to actual capabilities.
5. Report local implementation separately from release acceptance.
   The full-working-day observe-mode trial requires a later explicitly authorized live installation and elapsed real use.

## Status

- [x] Read the design, roadmap, compliance policy, and existing skeleton.
- [x] Verify six idle Codex workers in the Yoke workspace.
- [x] Dispatch all six ownership-scoped assignments and confirm activity.
  All six reported `working` in the Yoke workspace after dispatch.
- [x] Reconcile interfaces and dependencies.
  Worker handoffs document agreed storage, config, engine, install, and update APIs.
- [x] Integrate and verify Core locally.
- [x] Synchronize public documentation and report remaining acceptance gaps.
- [ ] Authorized real-world observe-mode trial for a full working day.
- [x] Implement and verify Workflow 0.3 locally, including approved checks and independent review.
- [x] Implement and verify Memory 0.4 locally, including portable import/export.
- [x] Implement Surfaces 0.5 and verify its principal browser journeys, responsive layouts, and accessibility checks.
  Lighthouse scores and approved screenshot-baseline comparison remain unproven release acceptance items.
- [x] Use a strong, recognizable yoke icon in the console and favicon, as requested by the user.
- [x] Redesign the rejected blocky yoke mark with smoother, balanced geometry; refreshed app screenshots show the replacement.
- [x] Fix the console select box styling and interaction to match the supplied theme.
  w5 selected Denied, applied the filter, and confirmed updated results; root inspected desktop and 390 px screenshots.
- [x] Apply the user's supplied dashboard reference across the console: pale lavender/mint shell, white rounded surfaces, indigo outline icons, compact sidebar, and pastel chart accents.
  This explicit visual direction supersedes the initial warm paper/teal aesthetic.
- [x] Implement and verify Presets 0.6 locally.
- [x] Report real-use acceptance periods separately from code completion; do not simulate elapsed days or remove the existing harness without explicit authorization.
- [x] Prepare the current version for user-authorized delivery: update README, maintain a generated changelog, and pass final local checks.
  Commit and push are now explicitly authorized; branch switching, tagging, release publication, and live installation remain outside this request.
  The Git history and remote branch record the subsequent commit and push outcome.

## Latest worker status

Core, Workflow, Memory, Presets, statusline, installer, and console implementation are integrated.
Focused CLI suites passed 7/7 Core and 5/5 roadmap journeys; console tests passed 6/6.
After migration 006, four storage tests and the seven Core CLI journeys passed, and CLI clippy completed with warnings denied.
The Sessions browser fixture improved from 324.2 ms to 18.3 ms click-to-frame across 100,006 decision rows.
All 24 page/theme/width combinations have zero detected accessibility violations and zero document overflow.
w3's final bounded review found no unresolved high-severity blockers in migration 006 or console transport/authentication.
The orchestrator freshly reran workspace formatting, whitespace checks, and the license/source gate successfully.
License warnings concern unused allowlist entries only.
Native post-migration release compilation passed; final package evidence is recorded in w6's handoff.
The native archive checksums, embedded notices/assets, schema 6 initialization, and temporary-home runtime checks passed.
Thirty release statusline launches measured 4.314 ms median and 4.960 ms maximum without database mutation.
No-mistakes is installed but not initialized in this repository; its pipeline was not run and no branch was created to satisfy its feature-branch convention.
Delivery uses the existing project checks and ownership-scoped worker reviews.
Real day/week use, three non-native builds, hosted release execution/publication, Lighthouse scores, and approved screenshot-baseline comparison remain unproven.
These limits do not imply that a release was published or that live agent settings were installed.

## Expanded roadmap assignments

The user explicitly requested all remaining work and all workers on 2026-10-13.
The following responsibilities extend original ownership without transferring existing files.
Each worker must publish its next API contract in its handoff before changing shared integration assumptions.

| Worker | Additional ownership | Outcome |
|---|---|---|
| w1 | `crates/yoke-core/src/memory.rs`, `crates/yoke-core/src/memory/` | Facts, recall, digest, portable export/import, central schema migrations and storage interfaces |
| w2 | `crates/yoke-core/src/plan.rs`, `crates/yoke-core/src/plan/` | Plan files, frozen approval, transitions, fingerprints, actual check execution |
| w3 | `crates/yoke-core/src/review.rs`, `crates/yoke-core/src/review/`, `crates/yoke-core/src/stop_guard.rs` | Independent review, bounded stop guard, reusable bounded process execution |
| w4 | `crates/yoke-cli/src/workflow.rs`, `crates/yoke-cli/src/memory.rs`, `crates/yoke-cli/src/presets.rs` | Central CLI/hook wiring, shared manifests, integration acceptance and explicit capability reporting |
| w5 | `crates/yoke-cli/src/console.rs`, `crates/yoke-cli/src/console/` | Local authenticated console, product design, browser interactions, accessibility and responsive proof |
| w6 | `crates/yoke-core/src/preset.rs`, `crates/yoke-core/src/usage.rs`, `crates/yoke-cli/src/statusline.rs` | Presets, transcript usage, bounded fast statusline; coordinate installer statusline support with w5 |

w1 retains sole ownership of schema migration files and store internals.
Other workers submit their schema and storage API requirements to w1 and w4 before implementation.
w4 retains sole ownership of shared manifests, lockfile, module declarations, CLI entrypoints, and hook dispatch.
w5 alone may operate one automation browser, using explicit session ownership and temporary fixture state.
After w6 completed statusline runtime verification, installer statusline integration was reassigned from w5 to w6 to keep w5 focused on the console.
w6 may edit `install.rs` and `install/` only after w5 explicitly acknowledges the ownership handoff and stops editing them.
Following user rejection of the first icon, w6 owns `console/yoke.svg` after w5 acknowledges the asset handoff; w5 retains its rendering and browser checks.
Following the concrete tiny_http blocking-body finding, w2 owns a new bounded `console/http.rs` transport and inline tests after agreeing its API with w5.
w5 retains `console.rs` and `events.rs` integration, all UI/select-box work, and the only browser lane.
No worker may create another browser or invoke real headless model runs without coordinating resource usage with the orchestrator.
Tests for reviewer/digest commands use fixture processes; real model acceptance remains a separately identified check.
The orchestrator owns README, design corrections, starter config, combined review, and final readiness reporting.

## Orchestrator verification notes

Core review found enforced rewrites recorded as `allow`; the integration owner was assigned the correction and a focused logged-outcome assertion.
README and starter configuration now describe trusted snapshots and the default observe behavior.
The starter no longer silently suppresses formatter errors or assumes every repository uses Rust formatting.
The design records current agent-specific rewrite contracts and Codex SessionEnd support, verified against official hook references.

The host ran low on disk space during a temporary cargo-deny source build.
The failed temporary build and rebuildable Yoke incremental cache were removed, with no source or user settings deletion.
Workers coordinate focused builds through w4 with incremental compilation disabled and two build jobs.
An official prebuilt cargo-deny 0.20.2 archive was downloaded and its SHA-256 matched GitHub's release-asset digest.
`cargo-deny check licenses sources` then exited 0 with `licenses ok, sources ok`; warnings concerned unused allowlist entries only.
That gate must be refreshed after the expanded dependency graph settles.

## Integration review queue

| Item | Owner | State |
|---|---|---|
| Log actual rewrites as rewrite; avoid duplicate decision on output failure | w4 | Fixed; focused RED/GREEN recorded in w4 handoff |
| Import dry-run must not create a destination home, DB, or migration | w4 / w1 | Implemented; w4 reports fresh-home CLI dry-run and mapped repeat-import passing |
| Match fingerprint output cap to the bounded process runner | w2 / w3 | Corrected to 16 MiB; w2 real-Git focused workflow tests passed |
| Retain documented `run` key in plan TOML and preset templates | w2 / w6 | Implemented in plan serde and templates; template test queued |
| Review untracked code as well as tracked changes | w3 | Implemented; focused review tests passed |
| Bound memory-context reads; record Codex apply_patch facts; preserve provenance on undo | w1 / w5 | Focused memory tests and browser undo/provenance journey passed |
| Bound export allocation before loading full records; reject blocking special-file inputs | w1 | Implemented and focused memory tests passed |
| Preserve session filtering across console form submissions and pagination | w5 | Browser Apply/Older/Latest preserved session filter |
| Provide rollback SQL and verify reverse migrations only in disposable databases | w1 | Paired scripts implemented; disposable rollback/reupgrade fixture passed |
| Reject special-file digest transcripts before blocking I/O | w4 | FIFO regression reproduced and passed after fix |
| Bind standalone review evidence to both pre/post fingerprints and propagate void/error | w2 / w4 | Fixed; focused core regression and real CLI changed-tree fixture passed |
| Avoid Stop state changes before a later hook failure; schedule digest only after persisted decision | w3 / w4 / w1 | Atomic guard/fact/decision commit implemented; six guard tests and focused CLI regression passed |
| Bound console trust previews and action-body reads; handle memory ID reuse during Undo | w3 / w5 / w1 | Implemented; six console tests, memory regression, and browser action checks passed |

Latest live inspection confirms all six workers continued their expanded assignments.
w4 reports isolated CLI memory migration, idempotent import, no-write fresh-home dry-run, statusline wrapping, and real shell-plan workflows passing.
w2 reports seven focused plan tests passing, including real Git and SQLite fixtures.
w3 reports engine 13/13, review 5/5, stop guard 4/4, and core clippy passing.
Its real Codex launch with deliberately empty authentication failed closed on HTTP 401; this is failure-path evidence only.
w3 was asked to check an existing authenticated context without inspecting or copying credentials before concluding successful acceptance is unavailable.
That authenticated real reviewer subsequently passed in 8.3 seconds with exit 0 and valid empty findings.
w1 was assigned a sequential authenticated synthetic-data digest acceptance check through the same bounded runner.
w2 and w3 were reassigned focused read-only integration reviews, preserving existing file ownership.
w5 has a compiling console prototype and is using the single headed browser lane; full console and icon acceptance remain unfinished.
The user explicitly requested a strong yoke icon, assigned to w5 for the console brand and favicon.
After coordinated cleanup, the latest disk check showed 3.0 GiB available.

Once code compiles, w3 may run one bounded real reviewer check against a disposable read-only fixture, with YOKE_OFF=1 and a maximum 120-second timeout.
Any real digest acceptance must be coordinated sequentially with that check.
This permits exercising Yoke's actual external command integration, not new orchestration agents or changes to live settings.
