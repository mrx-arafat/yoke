# Core worker w5: installation

## Expanded Surfaces contract and working plan

Additional exclusive ownership: `crates/yoke-cli/src/console.rs` and `console/`, including browser fixtures/artifacts.
Console entry point: `console::run(home: &Path, project: &Path, open_browser: bool) -> anyhow::Result<()>`.
Request w4 add `mod console`, `tiny_http`, and `pulldown-cmark` after permissive-license checks; all assets embed at compile time and require no frontend build or network.
CLI should expose `console --no-open` for an explicit owned browser and temporary-home verification.
Request w1 bounded cursor decisions read with stable row IDs, session summaries, change revision, single-memory lookup and undo/restore support for forgotten memories.
Request w2 plan list/detail/checks/transitions/review read contract and browser human approve/resume boundary.
w6 supplies usage summaries; installer statusline wrapping stays disabled until the real statusline runtime passes verification.

1. Implemented and browser exercised: decisions plus command-palette prototype on the real local HTTP server and SQLite fixture.
2. Implemented: authenticated routes, CSRF-safe mutations, SSE, all six real data pages and safe content rendering.
3. Implemented and principal paths exercised: keyboard/responsive/themes, inline confirmation and bounded undo.
4. Focused checks and 24 page/theme/width captures complete; four concrete accessibility/overflow failures fixed and rechecked.
5. Installer transferred to w6 after explicit safe-to-edit acknowledgment; no in-flight installer changes or tests.
6. Icon exclusively transferred to w6; revised curved indigo/white mark integrated and inspected at 16/32 px in the existing browser.

Prototype browser evidence: headed Playwright session `yoke-w5`, 100,005 seeded decisions plus lifecycle row, 50 rendered rows, row expansion and older/latest navigation exercised.
At 390 px the document scroll width and client width were both 390; measured first paint and first contentful paint were 120 ms.
The fixture initially used a noncanonical macOS project path; corrected canonicalization before the large-ledger check.
Browser reproduced visible hidden notices; fixed hidden-selector specificity, awaiting rebuilt embedded assets.
Preferred chrome-devtools-axi failed snapshot with missing pageId and its owned session was stopped before the Playwright fallback.
Full accessibility, approved screenshot baselines, and all-page performance acceptance remain unproven.

Current design follows the user's supplied reference: pale lavender/mint shell, rounded white panels, indigo outline navigation icons, dark readable type, and lavender/sage usage bars.
It supersedes the initial warm paper/teal prototype.
Light and dark tokens are separately defined in OKLCH.
Superdesign instructions were read; its CLI is absent and no design service is installed or contacted, per user constraint.

## Surfaces handoff and fresh evidence

Console API remains `run(home: &Path, project: &Path, open_browser: bool) -> anyhow::Result<()>`.
All assets are original repository-native Yoke assets; no third-party JS library or font is bundled.
Platform fonts are local fallbacks.
The standalone temporary axe-core package is only a browser verification tool, not a product dependency.
There are no requests for external page assets.

Ownership boundaries now exclude `install.rs`, `install/**`, and `console/yoke.svg` (w6), and `console/http.rs` (w2).
w5 integrates the bounded original HTTP transport and owns the other console files, tests, browser fixtures and artifacts.
tiny_http was removed by w4 after its unbounded request-body drain behavior was confirmed in its local source.
The new transport has bounded connections, absolute read/write deadlines, request byte limits, strict framing rejection and connection-close responses.
SSE has a separate four-stream limit, one persistent store per stream, a 55-second reconnect cycle and the transport's bounded stream writer.
Authentication applies before every route, asset, image and event stream; mutation requests additionally require same-origin Origin, CSRF header and JSON content type.
Approval retains the exact prepared header/revision server-side; forget undo retains the full provenance snapshot for 30 seconds and consumes the token only after successful restoration.
Screenshot reads use nonblocking/no-follow descriptors, regular-file checks, containment revalidation, a 10 MiB cap and image magic checks.
Trust previews use nonblocking/no-follow descriptors and a capped 1 MiB read.

Commands run with the coordinated shared target and `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2`:

- `cargo build -p yoke-cli --bin yoke --locked --quiet`: exit 0 after transport integration and again after the bounded UI fixes.
- `cargo test --locked -q -p yoke-cli console::`: exit 0, 6 passed, 0 failed, 29 filtered, 0.16 seconds (three transport tests, auth boundary, memory action undo/scope, safe content rendering).
- `node --check crates/yoke-cli/src/console/console.js`: exit 0.
- Owned Rust sources formatted with rustfmt; w4 owns final workspace lint.

Headed Playwright session `yoke-w5` exercised real temporary-HOME CLI servers:

- Decisions: 100,005 synthetic rows plus lifecycle event, 50 DOM records, expand, filter, older/latest pagination; `session=fixture-session` retained throughout Apply/Older/Latest.
- Plans: exact-header/digest inline confirmation, approve to `approved`, terminal fixture transition to waiting, browser Resume to `building` with human transition.
- Memory: pin, share, inline forget, undo retaining pinned/shared state; successful share explicitly requires human `yoke trust` before changed knowledge takes effect; Dismiss hides the notification.
- Lightbox: two explicit synthetic check artifacts containing real UI screenshots; open, ArrowRight, decoded second image, Escape close all passed.
- Icon/select: revised SVG visible in rebuilt app; 16/32 px crops inspected; Outcome selection and Apply changed the ledger; theme light/dark exercised in the page matrix.

`artifacts/browser-pages.json` preserves the initial 24-view report.
`artifacts/browser-pages-final.json` combines the initial 20 passing views with the four affected rechecks after fixing Usage scroll-region keyboard access and Trust long-path overflow.
All 24 combinations (six pages, light/dark, 1440/390 px) have zero observed axe-core 4.14.0 WCAG 2/2.1/2.2 AA violations and zero document overflow after the focused fixes.
This automated result does not establish complete WCAG conformance.
The fixture omits real Git check execution; synthetic screenshot rows are explicitly labeled as fixtures.

Actual images for review:

- `crates/yoke-cli/src/console/artifacts/refined-icon-select-desktop.png`
- `crates/yoke-cli/src/console/artifacts/refined-icon-select-mobile.png`
- `crates/yoke-cli/src/console/artifacts/usage-light-1440.png`
- `crates/yoke-cli/src/console/artifacts/trust-light-390.png`
- `crates/yoke-cli/src/console/artifacts/yoke-icon-16.png` and `yoke-icon-32.png`

Performance: prototype first paint and first contentful paint were 120 ms; shipped JS is 12,080 bytes / 3,745 bytes gzipped before the small Dismiss addition.
Most route samples were about 32 ms, but Sessions measured a real cold 324.2 ms click-to-frame and 310.2 ms fetch duration over 100,006 decisions.
w1 reproduced the cold indexed count and implemented migration 006's maintained count.
Final existing-fixture browser retest after normal writer migration: 100,006 decisions displayed, 18.3 ms click-to-frame, 7.1 ms request duration and 6.5 ms response interval.
The navigation miss is resolved.
The same run reported 204 ms first paint with axe's large init-script injected; this instrumented sample is retained honestly and is not proof of universal sub-200 ms first paint.
No speculative console optimization was applied.

Additional final interactions: Trust unified/split mode buttons changed the actual diff mode; final bounded-transport memory pin/share/forget/undo/Dismiss passed; explicit synthetic screenshot fixture opened and ArrowRight/Escape worked.
Final source freeze: console implementation complete; remaining release acceptance limits above remain explicit rather than checked off.

Impeccable ran on bounded sanitized rendered Decisions HTML (exit 2, valid JSON).
Advisories: hidden lightbox image has no src until opened; compact 11 px rail labels; type hierarchy; timestamp digits misidentified as numbered section markers.
The user requested no further visual redesign after inspecting the updated screenshots.

Unproven release acceptance: Lighthouse scores (CLI absent, available tool inventory has no Lighthouse/DevTools tool, current sole Chrome uses a debugging pipe rather than an attachable port), comparison against approved per-page screenshot baselines, exhaustive loading/error/reconnect state journeys, and real day/week soak.
No second browser was opened for these missing scores.
No live installation, user settings change, release publication, Git write or user binary replacement occurred.

## Public API and dependencies

Owns only `crates/yoke-cli/src/install.rs`, `crates/yoke-cli/src/install/`, and this note.
Implemented API: `install(&InstallOptions) -> anyhow::Result<InstallReport>` and `uninstall(&InstallOptions) -> anyhow::Result<InstallReport>`.
`InstallOptions { claude_settings, codex_hooks, codex_config, state_dir, executable: PathBuf, agents: Vec<Agent>, dry_run: bool, statusline: bool }` uses explicit absolute paths.
`Agent` has `Claude` and `Codex` variants.
`InstallReport { changes: Vec<FileChange>, warnings: Vec<String> }` returns each settings path with its `before` and `after` text as `Option<String>`; `None` means absent.
The CLI can print these changes as a dry-run diff.
No function discovers HOME or writes live settings in tests.
Worker w4 owns module registration and dependency changes.
Dependencies supplied by w4: serde derive, serde_json, anyhow, toml_edit 0.22.27, and tempfile 3.27.0.
No core module dependency or module declaration changes made by w5.

## Verified public contracts

Checked official documentation on 2026-10-13.
No competitor source or ambient competitor instructions were used as implementation material.

| Core event | Claude | Codex | Installed |
| --- | --- | --- | --- |
| PreToolUse | Supported | Supported | Both |
| PostToolUse | Supported | Supported | Both |
| UserPromptSubmit | Supported | Supported | Both |
| Stop | Supported | Supported | Both |
| SessionStart | Supported | Supported | Both |
| PreCompact | Supported | Supported | Both |
| SessionEnd | Supported | Supported | Both |
| Notification | Supported | Not in documented Codex event set | Claude only |

Sources:

- https://code.claude.com/docs/en/hooks - Claude settings location, event groups, command handlers, omitted matcher, and disabling controls.
- https://learn.chatgpt.com/docs/hooks - Codex local JSON/TOML hook locations, event support, feature key, trust review, and SessionEnd timeout.
- https://learn.chatgpt.com/docs/config-file/config-reference - canonical `features.hooks` and deprecated `features.codex_hooks` alias.

Claude uses its supplied settings JSON; Codex uses its supplied hooks JSON plus config TOML.
Both use `hooks.<Event>[]` containing a `hooks` command-handler array.
Omitting matcher covers all occurrences.
Codex currently documents SessionEnd, correcting the draft design.
The installer sets its SessionEnd timeout to three seconds.
New Codex configuration uses `features.hooks = true`.
An existing deprecated alias is preserved and enabled rather than guessing precedence between conflicting aliases.
Conflicting alias values fail before writes.
Codex requires the human to review new hooks with `/hooks`; installation never writes hook trust or bypasses it.
Managed policy can still prevent local hooks from running.
This supports local Codex execution, not cloud orchestration or hosted-tool coverage.

## Progress

Read README, compliance policy, design, and Core execution ownership.
Implemented installation, uninstall, JSON-preserving edits, Codex feature edits, durable ownership receipts, timestamped backups, and atomic per-file replacement.
The command uses a shell-quoted absolute executable path, including paths with spaces and apostrophes.
Receipts are persisted before settings writes and retained until uninstall succeeds.
Only exact newly added entries are owned; pre-existing matching hooks and other `yoke` commands are not adopted.
Later unrelated settings and hooks survive uninstall, including an unrelated handler appended inside an owned group.
An untouched install/uninstall round trip restores original file bytes and removes newly created settings files.
Settings edits preserve unchanged hook entry bytes; JSON container whitespace can change during installation or selective uninstall.
Codex configuration comments survive feature changes.
Dry-run performs no writes, including no directories, locks, backups, or receipts.
Statusline is absent until 0.5; `--statusline` returns an error before writes.
All-agent preflight rejects malformed JSON, duplicate JSON keys, non-boolean feature flags, conflicting aliases, and symlink settings targets.
Uninstall without receipts is a no-op, never a command-prefix deletion sweep.

## Recovery and limits

Settings replacement is atomic per file, not a cross-file filesystem transaction.
If interrupted between file writes, retry installation or uninstall using the retained receipt and backups.
A crashed installer can leave `install.lock`; verify no installer is running before manually removing that stale lock.
Yoke checks for concurrent changes before replacing each file, but another non-cooperating process can still race the final rename.
Duplicated owned entries or modified owned handlers/matchers cause a conservative refusal rather than deletion of ambiguous user data.
Restore the receipted entry before retrying uninstall in that case.
Backups intentionally remain after uninstall; empty directories may remain.
Settings and serialized ownership records are bounded to 8 MiB.
The installer preserves existing inline Codex hook tables; when hooks.json and inline hooks coexist, Codex merges them and may warn.
No live agent session, live installation, or full-working-day trial was run.

## Evidence

Initial focused Cargo test attempted while peer modules were incomplete: compilation blocked on missing peer-owned modules, with no changes to those modules.
Isolated `rustc --edition=2021 --test` harness compiled the owned module against the existing Cargo-built anyhow, serde, serde_json, tempfile, and toml_edit dependencies.
RED: all three initial behavior tests failed with `installation not implemented`.
GREEN: six focused installer tests passed, zero failed, in 0.49 seconds after the final behavior changes, compiled with `-D warnings`.
Covered round-trip/idempotence, dry-run/statusline zero writes, preservation of later edits, unowned matching hooks, legacy feature restoration, malformed/ambiguous ownership refusal, interrupted receipt recovery, and symlink refusal.
Every fixture uses `tempfile::TempDir` and passes explicit paths; no test accesses the actual user home.
Ran rustfmt only on w5-owned Rust files.
`cargo test -p yoke-cli install::tests --quiet` was attempted again and exited 101 because peer-owned `commands.rs` and `hook.rs` were still absent.
After w4 supplied the CLI modules, `cargo test -p yoke-cli install::tests --quiet` exited 0: six installer tests passed, zero failed, six other unit tests filtered out, in 0.43 seconds.
The integration target ran zero tests with all five filtered out; w4 retains ownership of `core_workflow` verification.
No live settings, binaries, or Git state modified.

Exact isolated check, run from the Yoke root in zsh:

```sh
testdir=$(mktemp -d)
args=()
for dep in anyhow serde serde_json tempfile toml_edit; do
    files=(target/debug/deps/lib${dep}-*.rlib)
    args+=(--extern "$dep=${files[1]}")
done
print "#[path = \"$PWD/crates/yoke-cli/src/install.rs\"] mod install;" |
    rustc --edition=2021 --test -D warnings - -L dependency=target/debug/deps "${args[@]}" -o "$testdir/install-tests"
"$testdir/install-tests" --quiet
```

Next owner action: w4 runs the temporary-home real CLI install/dry-run/uninstall integration scenario.
