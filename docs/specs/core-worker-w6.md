# w6: updater and release packaging

## Current authoritative handoff

The final post-migration006 native Apple Silicon release build and local packaging are complete at version 0.1.0.
The shared `target/release` directory was reused with the release notice cfg, `CARGO_INCREMENTAL=0` and `CARGO_BUILD_JOBS=2`; both authorized builds exited 0 and the Cargo slot is released.
Current binary: `target/release/yoke`, 8415648 bytes, SHA-256 `b3e32bfe234d57f10ef46f28b54cae7a67cd5709ed38868cfc13fdbc1555df2e`.
Native package directory: `/var/folders/hd/w62gtt492l9bqnn9gc6bwsy00000gn/T/yoke-w6-final-release-whc2qv4b/native-post-migration006`.
Archive: `yoke-aarch64-apple-darwin.tar.gz`, 3194268 bytes, SHA-256 `4abb93a628257df9a2769117ae62e56c5863c953ebab8ebab85dcd0836ad54a0`.

Verified current evidence:
- Raw executable, archive and notice sidecar checksums match; the archive contains the exact binary, executable mode and both own licenses plus dependency notices.
- The entire 777190-byte combined license bundle and current console CSS, JavaScript and SVG are embedded in the native binary.
  Third-party sidecar alone is 764728 bytes, SHA-256 `e829055d7c0ff0a7eb201f055bedd4cd7a347e8414e3298ded6089a3c35654c3`.
- Temporary-HOME release runtime reports 0.1.0, initializes schema 6, preserves identical wrapper stdin, creates no state on a fresh statusline call, and leaves existing database filenames, bytes and modification times unchanged.
  Thirty actual release statusline launches: median 4.314 ms, p95 4.651 ms, max 4.960 ms; 30/30 under 15 ms.
- Build-input hashes match the captured migration006 sources and embedded assets; generated browser logs are excluded from source-freeze comparison.
  Root accepted w5's actual Sessions browser result: 18.3 ms click-to-frame, 7.1 ms fetch over 100006 decisions.
- Focused lane tests: preset 4/4, usage 3/3, statusline 3/3, installer 8/8 plus the later interrupted-upgrade regression 1/1, packaging 2/2.
  Root/w4 owns the final integration, clippy and browser acceptance evidence.

Limits remain explicit: the other three native target builds/hosted release workflow, real self-update, publication and day/week soak have not run.
All four target notice graphs were generated successfully; w4 reported the current cargo-deny license/source gate passed.
Unknown usage pricing remains labeled unknown, and legacy WAL databases without necessary sidecars remain unavailable to the no-write reader until a normal writer opens them.
No live settings, installed binaries, version, tag or publication was changed by w6; root alone handles authorized Git writes.
Detailed generated build/package evidence is in `post-migration006-evidence.json` beside the native package directory.

## Historical implementation and verification evidence

The sections below preserve chronology; their pending/held statements describe earlier checkpoints and are superseded by the current authoritative handoff above.

## Bounded icon redesign

w5 explicitly transferred only `console/yoke.svg` before edits; CSS, renderer and browser remain w5-owned.
Replaced the rejected crossbar/double-square-U with an original white curved-y silhouette on an indigo tile, using broad rounded strokes and a curved descender.
The reference informed only indigo/white color direction, not copied icon geometry.
The asset retains `viewBox="0 0 32 32"`, is 295 bytes, and has no scripts, styles, external resources or dependencies.
XML structure validation passed.
w5 subsequently inspected actual browser captures at 16/32 px and reported distinct curves, no clipped strokes and no collapsed internal gap.
Evidence paths are `crates/yoke-cli/src/console/artifacts/yoke-icon-16.png` and `yoke-icon-32.png`; w6 did not operate a browser.
No browser or build was run for this change; final release remains queued behind source freeze.

## Final expanded release refresh

Expanded snapshot native compile completed successfully before the approved migration006 Sessions optimization.
Exact command: `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 YOKE_RELEASE_NOTICES=/var/folders/hd/w62gtt492l9bqnn9gc6bwsy00000gn/T/yoke-w6-final-release-whc2qv4b/THIRD_PARTY_LICENSES RUSTFLAGS='--cfg yoke_release_notices --check-cfg=cfg(yoke_release_notices)' cargo build --offline --locked --release -p yoke-cli --quiet`.
Shared `target/release/yoke` is 8415648 bytes, SHA-256 `b8c4edb68c2eaa6c316e18d59660d958ccde0c4760f1d59f9644bbcf9c3f532a`, and runs `--version` as 0.1.0.
The full 777190-byte notice bundle and exact current CSS/JavaScript/SVG bytes are present in the binary.
All 111 captured source files still matched immediately after compilation; snapshot SHA-256 is `5a307370838598f474dd9700e1f70bfd6285d6a67123583793f0d5e59f744250`.
Disk was 2.7 GiB free immediately before this build.
Released the compile slot and source hold to w1 immediately after exit 0.
Final packaging is deliberately held for w1's migration006 focused GREEN and browser timing, followed by an authorized incremental release rebuild/repackage of the changed sources.
This binary is expanded-snapshot evidence, not the final post-optimization release artifact.

Final preparation, before build authorization: host verified as `aarch64-apple-darwin`; current free disk 2.2 GiB; stable lock hash unchanged.
Generated the complete embedding file through the release script at `/var/folders/hd/w62gtt492l9bqnn9gc6bwsy00000gn/T/yoke-w6-final-release-whc2qv4b/THIRD_PARTY_LICENSES`.
It includes Yoke's own license texts plus all native third-party notices: 777190 bytes, SHA-256 `ed743cadcedf357849f6dbe9a521a14bf1c0ff3c9768de15d3763d6316173061`.
`prebuild-evidence.json` in that same owned temporary directory records the host, lock hash and notice hash.
No build started; this is a staging directory for small generated notices/package evidence, not a duplicate Cargo target.
Awaiting explicit post-browser acceptance build slot; shared `target/release` only, `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2`.
Final validation will compare compiled notice and console-asset bytes, execute version/statusline with temporary HOME, and validate the native archive/checksums without publishing.

Current post-tiny_http notice audit (no compilation): generated notices in memory from offline locked Cargo metadata for all four targets.
`Cargo.lock` SHA-256: `440d5d3a726c0441539ad8835bf5f9171fc18ac03c84611b4f3a01de7e893c4c`.
Both Apple targets resolve 72 third-party packages and 764728 notice bytes, SHA-256 `e829055d7c0ff0a7eb201f055bedd4cd7a347e8414e3298ded6089a3c35654c3`.
Both GNU Linux targets resolve 73 third-party packages and 789037 notice bytes, SHA-256 `83cd8c84d7e782d7a9e2f5039cc62c8912b538b67f6770aa3b249d8f10607d83`.
Every resolved dependency has a generated notice section; tiny_http is absent from all four target graphs.
Counts exclude workspace crates and differ from the complete lockfile count because metadata is target-filtered.
w4 reports the actual prebuilt cargo-deny license/source gate passed on this updated graph with only unused allowlist warnings.
The hashes above cover generated third-party notices, before prepending Yoke's own license texts for embedding.
No generated artifact was manually edited or retained; no build, packaging or publication occurred in this audit.
Native release still awaits console HTTP compilation/browser checks, final lint and w4's explicit freeze/slot.

Reviewing workflow and notice packaging against expanded dependencies and embedded console assets.
The prior native release build below is Core-only evidence and predates the expanded modules; it does not validate the final expanded binary.
Current disk check reports 1.4 GiB available.
w4 explicitly reports source freeze is not ready: guard/decision atomicity, review caller binding and console/security work remain in flight.
Native release is on hold until w4 confirms freeze and grants a slot after debug checks.
Approved future strategy: existing shared target release profile only, no duplicate target directory; version remains 0.1.0.
Other three target builds remain CI-unrun; no publication is authorized.
Latest coordination: w4 reports 12/12 final CLI tests green, including atomic Stop and P2 review, but console transport/browser and final lint still prevent freeze.
The dependency graph is expected to drop tiny_http after w5 confirms its replacement.
The 77-package/824868-byte notice audit below predates that removal and must not be reused as final release evidence.
After w4 updates the manifest/lockfile and confirms freeze, regenerate target-filtered notices and rerun the license/source gate before the approved shared native release build.
Last reported free space remains 1.4 GiB; no additional cleanup has been performed by w6.
Interim expanded-graph audit used `cargo metadata --offline --locked --format-version 1 --filter-platform aarch64-apple-darwin` without compiling.
All 77 resolved third-party packages produced license/NOTICE sections, totaling 824868 UTF-8 bytes before Yoke's own license embedding.
This includes tiny_http 0.12.0, pulldown-cmark 0.13.4, pulldown-cmark-escape 0.11.0, ascii 1.1.0, chunked_transfer 1.5.0, httpdate 1.0.3 and unicase 2.10.0.
Their metadata declares MIT and/or Apache-2.0 alternatives; w4 separately records successful cargo-deny enforcement for the expanded graph.
The generator remains a notice collector, not an SPDX checker.
Embedded console CSS, JavaScript and SVG are read directly by Rust `include_str!`; there is no separate generated frontend bundle to copy into packaging.
w5 was asked to confirm asset provenance and any third-party attribution outside Cargo.
The existing native workflow and target-filtered generator require no structural changes for the expanded crates; final evidence must regenerate notices and validate their bytes plus embedded assets in the newly built binary after freeze.
Fresh packaging-only check: `PYTHONDONTWRITEBYTECODE=1 uv run --no-project python -m unittest discover -s scripts/release -p 'test_*.py' -q`, 2 passed in 0.023 s.
No native build or package was produced during this interim audit.

## New bounded installer assignment

w5 explicitly confirmed safe-to-edit ownership transfer before any installer edits; no edits or tests were in flight.
w6 now owns `install.rs` and `install/**`; w5 retains console only.
Implemented reversible statusline wrapping while preserving hook receipts, backups, idempotency and unrelated settings.
Existing version-1 receipts remain readable through an optional, defaulted statusline ownership field.
Original and installed `statusLine` values are recorded; unchanged full files restore exact original bytes, and selective removal preserves later user edits.
Existing hook-only installations can add statusline, and interrupted receipt-first additions recover without nesting wrappers.
Only Claude receives the documented `statusLine` command; Codex-only `--statusline` refuses before writes.
Existing command objects preserve padding and other options; unsupported/empty/oversized commands refuse before writes.
The installed shell command quotes both the executable and original wrapper command, with the existing callback receiving identical stdin.
Public installer API and dependencies are unchanged.
Verified official schema: https://code.claude.com/docs/en/statusline (`type: command`, `command`, optional padding/other fields).
Completed focused behavioral coverage and real temporary-HOME install/uninstall restoration in a w4-coordinated build slot.
No live settings, browser, Git writes or root documentation edits.

## Expanded roadmap contracts (publish before implementation)

Ownership now also includes core `preset.rs`, core `usage.rs`, and CLI `statusline.rs`.
No manifest, migration, CLI integration or console files will be edited by w6.
Installer ownership was separately transferred as recorded above.

- Presets: `preset::list() -> &'static [PresetInfo]`; `preset::add(path: &Path, name: &str, options: &PresetOptions) -> Result<AddOutcome>`.
  `PresetOptions { replacement: Option<String>, wrapper: Option<String>, command_prefix: Option<String>, acknowledge_rewrite_permissions: bool }`.
  Token-saver requires an explicit simple command prefix, an existing executable wrapper on PATH, and acknowledgment that a Codex rewrite bypasses its normal permission prompt.
  Claude rewrites preserve normal permission evaluation.
  Generated config is appended atomically without trusting it; repeated exact additions are no-ops and conflicting gate names fail.
- Usage: `usage::ingest(store: &Store, agent: &str, session: &str, project: &str, transcript: &Path) -> Result<Vec<UsageSummary>>`; `usage::summaries(store: &Store, project: Option<&str>, session: Option<&str>) -> Result<Vec<UsageSummary>>`.
  Schema request to w1: `usage_samples(agent TEXT, session_id TEXT, project TEXT, record_key TEXT, model TEXT, input_tokens INTEGER, output_tokens INTEGER, cache_read_tokens INTEGER, cache_write_tokens INTEGER, PRIMARY KEY(agent,session_id,record_key))`, nonnegative token checks, project/session index.
  Idempotent samples are parsed before a transaction; unknown prices yield null cost plus explicit unknown-pricing counts, never guessed prices.
  Transcript schemas and the model pricing table will be confirmed from bounded local structure-only inspection and official sources before implementation.
- Statusline: `statusline::run(wrap: Option<&str>) -> Result<()>`, with bounded stdin, one line output and an optional bounded wrapper receiving identical stdin bytes.
  Request w1: `Store::open_read_only(path) -> Result<Store>` without creation/migration, and `Store::status_snapshot(session: Option<&str>, project: Option<&str>, now: i64) -> Result<StatusSnapshot>` using indexed reads.
  Proposed `StatusSnapshot { mode: Option<String>, plan_slug: Option<String>, plan_state: Option<String>, recent_deny: bool, latest_version: Option<String> }`.
  Mode is the last observed decision mode, explicitly unavailable when no recorded decision exists; no fabricated enforce/observe state.
  Missing database is a normal empty status, and statusline must never call writable `Store::open`.
- Request w4: declare core `preset`/`usage`, CLI `statusline`; core production tempfile if needed for safe preset replacement; expose CLI commands and usage ingestion without modifying transcript files.
- Request w5: consume `usage::summaries` for console; enable reversible statusline wrapper installation only after the command has runtime evidence.

Working order: agree schema/APIs, implement presets, verify transcript/pricing inputs, implement usage/statusline, run focused tests and temporary-home CLI measurements, hand off remaining acceptance limits.

Expanded source progress: preset module is implemented, formatted and passed its coordinated focused tests.
UI templates use public `run` keys and a focused regression test deserializes both uncommented templates into the actual `plan::PlanHeader`.
Screenshot templates declare `artifacts = []` for actual paths and require Git-ignored output directories; declarations never exclude files from fingerprinting.
Token-saver descriptions, consent errors, comments and gate messages distinguish Codex permission bypass from Claude permission preservation.
Shared `engine::execute_bounded` is available and will be reused for statusline wrapping.
Final usage ingestion return type is `Result<Vec<UsageSummary>>` because one transcript can contain multiple models.
Summary fields are `agent`, `session_id`, `project`, `model` strings; four token counts as `u64`; `estimated_cost_usd: Option<String>` and `unknown_pricing_samples: u64`.
Requested nullable per-sample `cost_nano_usd` from w1 so missing pricing dimensions remain unknown rather than guessed.
No Cargo builds will run without w4 coordination.
After confirming no process used the owned `target/w6-release` directory, removed that authorized disposable output and reclaimed 206714613 bytes.
Available disk space immediately afterward was 652283904 bytes; reported to w4.

`usage.rs` and `statusline.rs` are implemented, formatted and passed coordinated compilation and focused tests.
Usage imports at most 32 MiB with 1 MiB lines from a regular, non-symlink JSONL file, rejecting malformed snapshots before starting a transaction.
Follow-up on 2026-10-14: the per-line limit was removed; typed accounting probes discard decoded message content while preserving usage on large assistant records, within the same 32 MiB whole-file bound.
Claude streaming snapshots deduplicate message ids; Codex cumulative counters become deltas and a counter restart starts a new segment counted from zero.
Only identifiers and counts are persisted, with identifiers redacted and all SQL values parameterized.
Costs use integer nanodollars from a compiled four-model Claude table, only when standard service, global inference, standard speed and any cache-write TTL split are present.
Missing dimensions, unlisted models and current Codex records remain explicitly unknown-priced.
These are API token-price estimates, not subscription invoices or tool charges.
Bounded structure-only inspection of two recent transcripts per agent confirmed the adapters without printing user content.
A second Claude inspection confirmed `standard` service/speed, `global` geography, and `ephemeral_5m_input_tokens`/`ephemeral_1h_input_tokens` cache breakdown keys.
Statusline reads existing state with w1's 5 ms busy timeout and one snapshot query; missing state creates nothing.
Its wrapper receives unchanged bytes through the shared runner with a 250 ms deadline, 4 KiB output cap and recursion prevention.
The under-15-ms target excludes optional user wrapper duration and time waiting for callback stdin EOF; real process timing is recorded below.
w1 confirmed plain read-only SQLite can create shared-memory sidecars and changed the owned store API to encoded canonical URI `mode=ro&readonly_shm=1`, without `immutable`.
Normal writers preserve WAL sidecars for later no-write readers.
Added a statusline fixture comparing every database-directory filename and byte before/after a refresh, then removing only the fixture's SHM file and asserting failure without file creation or mutation.
The fixture passed in the coordinated Cargo slot.
Existing databases lacking necessary sidecars can remain unavailable to statusline until a normal writer opens them; statusline never repairs them.

Expanded official sources:
- https://code.claude.com/docs/en/statusline confirms callback context percentage and session/workspace fields.
- https://platform.claude.com/docs/en/about-claude/pricing confirms model rates, geography, cache TTL and speed distinctions.
- https://platform.claude.com/docs/en/models/overview confirms model identifiers.
- https://developers.openai.com/api/docs/pricing and https://developers.openai.com/api/docs/guides/prompt-caching explain pricing dimensions; local Codex token-count rows do not establish all of them, so cost remains unknown.

Expanded validation so far: owned Rust files formatted with `rustfmt --edition 2021`; scoped `git diff --check` exit 0.
Expanded Cargo tests and real CLI timing completed in the authorized slot; exact evidence is below.
w4 reports the exported usage/statusline modules compile in the shared CLI and a fresh-home real statusline plus `cat` wrapper preserves stdin successfully.
w6 completed its focused tests after w3 and w1, then released the slot to w4.
Integration correction: valid empty or early transcripts now return an empty vector before any transaction, preserving previously imported usage rather than failing lifecycle hooks or deleting samples.
Malformed JSON, oversized input and invalid supported counters remain errors.
Added one regression covering both agents, empty/whitespace/early records, preservation of prior token counts, malformed JSON and the line-size limit.
The 2026-10-14 follow-up replaces the line-size rejection with coverage that large tool outputs and usage-bearing assistant records are both imported correctly.
The regression was authored before the fix but could not be executed for RED because the shared Cargo slot was held; the eventual focused GREEN run passed.

## Public API and dependency coordination

- CLI module: `update::run(check_only: bool, store: &yoke_core::store::Store) -> anyhow::Result<()>`, integrated by w4 with w1's confirmed store API.
- `update::check_latest(&Store) -> anyhow::Result<String>` exposes a one-request bounded check and persists `update.latest_version` only after successful validation.
- Check-only performs bounded metadata retrieval and records `update.latest_version`; scheduling and a last-attempt timestamp belong to w4.
- w4 declared `mod update;` and added CLI `serde`, `sha2` and production `tempfile` dependencies.
- No new HTTP dependency: invoke curl with HTTPS-only redirects, connect/overall timeouts, output caps, and an independent process deadline.
- Production repository is `mrx-arafat/yoke`, verified from `git remote -v`.
- Raw executable assets: `yoke-<target>`, matching `.sha256`, plus a bundled `yoke-<target>.tar.gz` containing the executable, Yoke licenses and generated dependency notices.
- Targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`.
- Validate release tags, exact expected asset names and canonical repository download URLs before downloading.
- Stage beside destination, verify checksum, preserve executable permissions, sync, then perform one atomic rename.
- Own only `crates/yoke-cli/src/update.rs`, `update/`, `.github/workflows/`, `scripts/release/`, and this handoff.

## Verified public sources

- https://docs.github.com/en/rest/releases/releases#get-the-latest-release : release JSON fields and latest stable release endpoint.
- https://curl.se/docs/manpage.html : timeout, protocol, redirect and output-size controls.
- https://doc.rust-lang.org/std/fs/fn.rename.html : same-filesystem rename semantics.
- https://docs.github.com/en/actions/reference/runners/github-hosted-runners : supported runner architectures.
- https://doc.rust-lang.org/cargo/commands/cargo-metadata.html : resolved dependency graph and package license-file fields.
- https://docs.rs/tempfile/latest/tempfile/struct.NamedTempFile.html#method.persist : atomic replacement of an existing destination, retaining the staging file on failure.
- https://github.com/actions/checkout : checkout action interface.
- https://github.com/actions/upload-artifact : artifact upload action interface.
- https://github.com/actions/download-artifact : artifact download and merge interface.
- https://github.com/astral-sh/setup-uv : pinned uv setup action and Python execution.
- https://cli.github.com/manual/gh_release_create : draft creation and `--verify-tag` contract.

The cargo-about documentation endpoint could not be fetched reliably and that tool was not used.
Instead, the generator reads Cargo's official metadata format and copies actual license/NOTICE texts from resolved package sources.

## Progress

Read README, COMPLIANCE and both Core specifications.
Published contract before implementation.
No live settings, binaries or network writes are authorized.
Implemented `update.rs`, bounded curl transport and six focused fixture tests.
The endpoint is fixed to the verified repository; release names/URLs are validated and selected assets cannot supply filesystem paths.
Each curl request has a 5-second connect timeout, 30-second curl timeout and 35-second parent deadline, with a capped reader independent of curl's output-size option.
Metadata is capped at 1 MiB, binary at 64 MiB and checksum at 256 bytes.
Regular builds have no embedded-notice cfg enabled; CI release builds embed generated license bytes through `yoke_release_notices` and `YOKE_RELEASE_NOTICES`.
w4 registered the custom cfg in the CLI manifest.
Packaging emits raw executable/checksum assets and license-bearing tar archives for four native runners.
Every packaged executable, archive and notice sidecar receives its own checksum.
The workflow creates drafts only; publishing still requires explicit human action and compliance review.

## Test evidence

- Transferred installer: `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test -p yoke-cli install::tests --quiet --locked`: 8 passed, 0 failed, 10 filtered, 0.81 s.
  The original six tests remain, with the obsolete statusline-unavailable assertion replaced by Claude dry-run/no-write and Codex-only rejection behavior.
  Two additional behavioral tests cover existing-command wrapping/exact restoration and hook-receipt upgrades/later user edits.
  Added an interrupted-upgrade assertion afterward and reran `install::tests::adding_statusline_to_hook_receipt_preserves_later_edits_and_user_override`: 1 passed, 0 failed, 0.30 s.
  Cargo slot released immediately; no separate target or release build used.
- Real temporary-HOME CLI installer: dry-run directory hashes unchanged; installed shell command split into the exact executable/statusline/--wrap/original-command arguments; execution preserved quoted text and raw stdin.
  Original padding retained, repeated install reported zero changes, uninstall restored the settings bytes exactly and removed the receipt, and originally absent settings returned to absent.
  Backups remain intentionally, following the existing installer contract.
- Expanded coordinated slot: `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo test -p yoke-core preset:: --quiet --locked`: 4 passed, 0 failed, 55 filtered, 0.03 s.
- Same environment, `cargo test -p yoke-core usage:: --quiet --locked`: 3 passed, 0 failed, 56 filtered, 0.05 s.
- Same environment, `cargo test -p yoke-cli statusline:: --quiet --locked`: 3 passed, 0 failed, 13 filtered, 0.27 s; integration binaries ran zero matching tests.
  Includes WAL directory-byte preservation and missing-SHM refusal without creation.
  Peer-owned console unused-field warning was reported to w5; no w6 compile warning appeared.
  Released the Cargo slot immediately to w4 for its queued regression, then w2.
- Real shared debug CLI, temporary HOME only, Python subprocess timer including startup: 30 statusline invocations, median 9.423 ms, p95 (29th sorted sample) 11.914 ms, max 12.862 ms; 30/30 under 15 ms.
  Existing database directory names, SHA-256 bytes and modification nanoseconds remained unchanged.
  Fresh-home `statusline --wrap cat` preserved identical stdin bytes and created no state directory.
  Optional wrapper time is outside the ordinary refresh target; the bounded timeout has separate fixture coverage.
- Real temporary-home CLI preset list/add: four presets listed; UI `run`/`artifacts` guidance present; original config bytes preserved; second addition unchanged; no implicit trust/database creation.
- `uv run --no-project python -m unittest discover -s scripts/release -p 'test_*.py' -q`: 2 tests passed; four target bundles, checksums, executable mode and nested NOTICE preservation exercised.
- `uvx ruff check scripts/release`: all checks passed.
- `uvx ruff format scripts/release`: formatted the two owned Python files.
- Ruby YAML parse succeeded and found exactly four workflow target entries.
- Actual Cargo graph, filtered for `aarch64-apple-darwin`: generated 750384 bytes of notices and packaged a local fixture executable successfully in a temporary directory.
- `cargo test -p yoke-cli update:: --quiet --locked`: final integrated run passed 6 tests, 0 failures; no broader tests executed.
- Prior to peer CLI integration, a standalone rustc harness against the freshly built core library passed the same 6 tests.
  A checksum-bypass mutation compiled only in the temporary harness caused the checksum-preservation test to fail as expected; working-tree production code was never mutated.
- `go run github.com/rhysd/actionlint/cmd/actionlint@latest .github/workflows/release.yml`: passed with actionlint v1.7.12, exit 0.
- Earlier Core-only build, before expanded modules: `cargo build --locked --release -p yoke-cli --target aarch64-apple-darwin --quiet`, with the documented release cfg and generated notice path in the former isolated `target/w6-release` output: passed.
  The resulting executable contains the entire 762846-byte generated bundle of Yoke's own and third-party notices, runs `--version` successfully, and packages successfully with the real release script.
  The current manifest version remains 0.1.0; w6 does not own version changes.
- `cargo build -p yoke-cli --quiet --locked`: passed.
  A real `yoke update --check` invocation with temporary HOME and a local fake curl recorded `v0.2.0` in SQLite.
  A second check with malformed metadata returned nonzero with stderr diagnostics while stdout was discarded, retained the previous database value, and left the executable SHA-256 unchanged.
- `git diff --check` restricted to w6 paths: passed.
  No `SHORTCUT:` markers exist in owned implementation files.

## Open integration and release limitations

Daily scheduling and detached-child lifecycle belong to w4 and are not verified by w6's tests.
w4 confirmed atomic interval claims from w1, detached stdio, `YOKE_OFF=1`, and stderr error capture; the latter is verified through the real check-only failure scenario.
No release exists locally to establish a prior asset convention; workflow and updater will define it together.
Only the native Apple Silicon release build ran locally.
The other three target builds, hosted runner execution, draft creation and publication remain unrun.
The local cargo-deny executable was absent; license/source enforcement is wired into the workflow and remains a CI/integration gate, not a claimed local pass.
The notice script itself does not evaluate SPDX expressions or select allowed alternatives; it gathers actual license and NOTICE texts from the target-filtered graph.
The release workflow enforces allowed SPDX alternatives through `cargo deny check licenses sources` before invoking the notice script.
Running the notice script alone does not enforce the license policy.
For `r-efi`'s `MIT OR Apache-2.0 OR LGPL-2.1-or-later` expression, MIT or Apache-2.0 satisfies the current allowlist without choosing LGPL.
This follows cargo-deny's documented expression evaluation at https://embarkstudios.github.io/cargo-deny/checks/licenses/cfg.html#allow and is not a claim that the checker ran locally.
Keep tempfile's secure default randomness; substring matching of `LGPL` is not an SPDX alternative evaluator.
Linux runtime compatibility below the Ubuntu 24.04 runner's libc baseline has not been tested.
Checksums detect corrupt downloads; they are not an independent release signature against repository compromise.
Atomic rename gives process-level replacement; power-loss durability of the parent directory is not claimed.
