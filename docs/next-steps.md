# Development handoff

Recorded 2026-10-18 at commit `9df0602`, after the current code, CI and documentation packages reached `main`.
This is the starting point for the next development session, not a release acceptance claim.
Track remaining acceptance in [issue #22](https://github.com/mrx-arafat/yoke/issues/22) and the [audit tracker #23](https://github.com/mrx-arafat/yoke/issues/23).

## State of main

`main` is at `9df0602`, following code commit `6a98f22` and CI/installer commit `a2b3428`.
Local combined verification passed all 41 checks in the default temporary layout and `TMPDIR=/tmp`.
[Installer run 39058112818](https://github.com/mrx-arafat/yoke/actions/runs/39058112818) passed all 11 jobs at `9df0602`.
[CI run 39058112836](https://github.com/mrx-arafat/yoke/actions/runs/39058112836) passed formatting, clippy, docs/scripts and dependency checks, but its macOS and Ubuntu test jobs failed `digest_rejects_fifo_without_waiting_for_a_writer`.
The fixture lacks a stubbed `claude` executable and stops at reviewer lookup before the intended regular-file refusal.
The reported local fix validates the transcript before reviewer lookup and passes the FIFO case with an empty PATH; a new commit and green hosted result are still required.
The earlier [Installer run 39052847800](https://github.com/mrx-arafat/yoke/actions/runs/39052847800) passed all 11 jobs at `d5d4ed5`, while its [CI run 39052847858](https://github.com/mrx-arafat/yoke/actions/runs/39052847858) failed on Ubuntu statusline width.
The earlier statusline failure has local regression coverage in the new source; the new hosted blocker is the reviewer fixture above.
No binary release is published, and these commits did not replace installed hooks.
The new source supports schema 15; the installed Linux field-test build remains `d5d4ed5`, schema 14.
All installed hook binaries must be upgraded together before opening their database with the new writable build.

Commits since `746cef9`, the state recorded after the first field test, newest first:

- `9df0602` docs: document gate environments, project moves and safe release acceptance
- `a2b3428` ci: run tests in a disposable home and add a hook benchmark
- `6a98f22` fix: close the remaining security findings and harden gates, review, install and usage
- `d5d4ed5` docs: add the verified user guide and refresh the field test ledger
- `85cad17` fix: build the newest main when no release exists, and test that in CI
- `0afac36` docs: document the security model, recovery paths, scale numbers and the observer
- `a42a20e` ci: harden the release and installer paths
- `6947257` fix: security hardening, failure-safe hooks, scale indexes and a richer observer
- `dea9c38` docs: record that enforcement blocks with real Claude Code and Codex
- `132f3b5` docs: record the first field test and refresh the development handoff
- `96112d8` chore: ignore the private .notes folder
- `8ce9b2d` feat: the installer gives Claude Code Yoke's statusline

The first-wave fixes are `250dd84`, `ca1cb8f` and `746cef9`; earlier commits back to `05310d7` are in `git log`.

## What is done

- **First field test.** Implementation fixes now cover all 79 findings across seven journeys; the last `usage_partial` notification fix (j4-3) is independently checked and committed in `6a98f22`.
  See the [field test record](field-testing.md).
- **Security.** A full audit found 29 findings; 23 are fixed, 5 partly fixed, none are open and 1 is declined.
  Partial items retain explicit implementation, documentation or release limits in the ledger.
  A session is pinned to the project it started in, gates run in the directory of the trusted `yoke.toml`, the guard protects the hook registration files and the executable, `YOKE_OFF` works only with the private token from `yoke pause`, `yoke install`, `uninstall` and `update` refuse agent processes, state is private, redaction covers more shapes, and injected memory is fenced and labelled as untrusted data.
- **Failure modes.** Hooks fail safely on malformed or oversized input, unreadable or locked state, killed processes and closed pipes.
  Observe mode keeps working when a project was last seen in observe mode and the state is unreadable.
- **Scale.** Migrations 013 to 015 add indexes, session anchors and exact usage summaries, and retention and notification catch-up run as background jobs.
  Migration 015 compatibility and rollback have focused schema-12-to-14 coverage.
  On a 740 MB fixture `yoke health` fell from 280 ms to 3 ms and `yoke trial report` from 1.9 to 3.6 s to 0.4 s.
- **Observer.** The statusline is width-aware with `--style compact` and `--style detailed`, and `yoke watch` is a live, read-only terminal view.
- **CI and installer matrix.** Every push to `main` and every pull request runs formatting, clippy, the full test suite on Ubuntu and macOS, `cargo deny`, the changelog check and the documentation check for links, anchors and dashes.
  The installer workflow runs the one-line installer on nine Linux images as root, as a user with sudo, and in a terminal where a driver answers the questions.
  No workflow runs the installer on macOS.
- **Release tooling.** Pushing a tag runs `release.yml`, which builds four targets on native runners and creates a draft release.
  Since `a42a20e` it accepts only `vX.Y.Z` tags, pins its tools, runs `cargo deny` in its own job, requires the tagged commit to be on `main` with a green CI run, publishes `SHA256SUMS` and build provenance, and builds reproducible archives.
  The [release runbook](releasing.md), [verifying releases](verifying-releases.md), the draft [0.1.0 release notes](release-notes-0.1.0.md), the uninstall guide and the security policy are written.

## What is verified

This is implementation evidence, kept apart from release acceptance.

- **Local integration and hosted CI.** The final local gate passed 41/41 checks in both temporary layouts.
  Current hosted CI fails the reviewer fixture on both platforms; the earlier `d5d4ed5` statusline failure remains historical evidence.
- **Installer matrix.** The new run passed all 11 jobs at `9df0602`; the earlier run also passed for `d5d4ed5`.
  Commit `85cad17` passes `YOKE_COMMIT` to each source build, checks the installed build and makes the interactive statusline assertion fail correctly (release-1 in the ledger).
- **Live enforcement with real agents.** On 2026-10-17, on the field-test server, a policy in enforce mode with one gate blocked a marker command in Claude Code and in Codex after `yoke trust`, an ordinary command ran in both, and the store held a `deny` with the gate name for each agent.
  That run used the build from before the second wave.
  See [enforcement verified with real agents](field-testing.md#enforcement-verified-with-real-agents-2026-10-17).
- **Historical large-database timings.** Measured on one 16 GB laptop with a 740 MB fixture of 500 thousand decisions: every hook event stays at 5 to 10 ms in the median, and a Stop with a 5 MB transcript takes 35 to 60 ms.
  These predate the current implementation; no new fixture above 50 MB was permitted in this wave, so the 700 MB target remains unverified.
  Current bounded debug-process hook medians are 16.7 to 17.8 ms, above the p50 below 10 ms target.
  Function-level notification timings do not substitute for complete hook latency.
  See [Scale](field-testing.md#scale).
- **Second-wave checks.** The audit's reproduction script flips from `VULNERABLE` to `SAFE` for SEC-1 to SEC-6 against a debug build.
  A kill storm of 360 hooks left the database intact, and 50 concurrent hooks all answered correctly.
  Every fix has focused tests, most written to fail first.
  See [second wave](field-testing.md#second-wave-2026-10-17).

The current Linux field test ran real Claude Code and Codex sessions against `d5d4ed5`: ordinary commands, trusted denials, settings protection and enforcement with `YOKE_OFF=1` passed.
The directory-change attempts kept their original hook payload cwd, so live session-anchor acceptance remains unproven.
See the [current installer and real-agent field test](field-testing.md#field-test-2026-10-17-current-installer-and-real-agents).
The release workflow has not run, and the console has not been driven by a person.
The full list is under [Not verified live](field-testing.md#not-verified-live).

## What is open

- **Findings.** The 154-row ledger has 145 fixed, 6 partly fixed, 1 open and 2 declined; [open items](field-testing.md#open-items) records the remaining limits.
  SEC-8, SEC-10, SEC-16, SEC-23, SEC-28 and C13 remain partial.
  S-6 is open: the schema-15 rehearsal found Codex samples without timestamps, so per-day totals omit them even though session totals include them.
  Release-2 is corrected in `9df0602`; safe native account/container execution remains unrun.
  The SEC-16 troubleshooting table and SEC-23 shell precheck proposal still need reconciliation.
- **Decisions.** The owner questions are under [Decisions and checks that need a person](field-testing.md#decisions-and-checks-that-need-a-person): the worktree memory identity, the linked-worktree trust rule, injection of notes by default, managed-settings install and the statusline priority.
- **Release gates.** The [gate status table](release-acceptance.md#gate-status) was last checked at `e2745ce` on 2026-10-16, and `release.yml` has changed since, so recheck it before relying on it.
  The release workflow has not run on all four targets, no release is published, and the compliance ticks, trademark search, observe-day and enforce-week trials, console acceptance and part of the install acceptance need a person.
- **Installed hooks.** A machine that already runs older hooks needs the coordinated upgrade below.
- **Free-model work.** Two audits produced no report and one finished without independent checking; see [free-model work](field-testing.md#free-model-work).

## Human-only steps

An agent cannot complete these and must not bypass their guards; record the exact command and stop.

- `yoke trust` and `yoke trust --global` approve a policy.
- `yoke console` starts the console.
- `yoke pause` prints the private token that lets a person start an agent without Yoke's hooks, or pauses them for a limited time.
- `yoke plan approve`, `resolve`, `acknowledge`, `resume`, `accept`, `cancel` and `reassign` decide a plan, and `yoke plan task done` does so for tasks owned by a human.
- `yoke notifications ack`, `yoke jobs retry` and `yoke project move` need a human terminal.
- `yoke install`, `yoke uninstall` and `yoke update` (without `--check`) refuse agent processes, as do `yoke remember --pinned`, `yoke memory pin` and a real `yoke memory import`.
- Approving the Yoke hooks in Codex with `/hooks`.
- Dispatching the release and installer workflows for the rehearsal, with the commands in [step 3 of the runbook](releasing.md#step-3-rehearse-on-all-four-targets).
  A manual run builds artifacts and creates no release.
- Enabling private vulnerability reporting for the repository, which `SECURITY.md` relies on and which is disabled today: in the repository settings, or with `gh api -X PUT repos/mrx-arafat/yoke/private-vulnerability-reporting`.
- Creating the protected `release` environment with required reviewers and protecting `v*` tags, which `release.yml` expects before it publishes a draft.
- Ticking [COMPLIANCE.md](../COMPLIANCE.md) and doing the trademark search.
- Running the observe-day and enforce-week trials for both agents, and the console acceptance pass.
- Publishing a draft release.

## Next work, in order

1. **Confirm hosted checks and reconcile partial items.**
   Commit and verify the digest input-validation ordering fix in CI on both platforms; local integration is green and Installer passed all 11 jobs.
   After CI is green, investigate and backfill recoverable Codex sample timestamps for daily attribution (S-6).
   Reconcile SEC-16's stale troubleshooting row and SEC-23's shell precheck proposal, then retain the memory and release policy decisions below.
   The corrected release-2 guidance requires a disposable account or container for shipped-artifact rehearsal.
2. **Run the second wave with real agents.**
   A person at the keyboard, on a fresh server or an isolated account: `yoke trust` and enforce mode in both agents, a `cd` out of the project and a planted nested `yoke.toml`, `yoke pause` and the token, a plan holding Stop, Codex hook approval after a reinstall, the statusline styles in Claude Code, and `yoke watch` in a terminal pane.
   Record it as a new dated section in [field testing](field-testing.md), starting from the list under [Not verified live](field-testing.md#not-verified-live).
3. **Make the owner decisions and finish the partly fixed rows.**
   SEC-8 still lacks trusted-only injection and memory authorship; SEC-10 and SEC-28 retain provenance, signing and maintainer-policy work.
   SEC-19 now has a trusted update opt-out; SEC-29's nonblocking hash descriptors passed the corrected FIFO regression, including a checked child test count.
4. **Coordinate the installed-binary upgrade.**
   Recheck which executable the installed hooks run and the state schema with `yoke health` before acting, then follow [upgrade notes](upgrade-notes.md): finish active checks, preserve a consistent database backup, replace every hook executable together, reinstall with the actual agent configuration directories, approve Codex hooks, and inspect `yoke health`.
   Do not open the real database with the new writable binary while old hooks still use it.
   An older binary against a newer database fails closed, which denies tool calls and blocks Stop even in observe mode; the installer script stops before replacing the executable in that case, so check `yoke health` yourself before a manual install.
   A rehearsal on 2026-10-15 with a debug build, an isolated `HOME` and an SQLite backup copy moved schema 6 to 12 in about 0.1 s, with the integrity and foreign key checks passing and rollback by restoring `state.db`, `state.db-wal` and `state.db-shm` together.
   Migration 013 was rehearsed on a clone of a 500 thousand decision database: about 9 s warm and 19 to 26 s cold, with a tested down script, and hooks that arrive meanwhile decide from read-only state without recording.
   Migration 014 is covered by migration round-trip tests; migration 015 has focused schema-12-to-14 upgrade, summary mutation and rollback tests.
   The orchestrator also rehearsed schema 14 to 15 on a state copy: notifications were retained, session totals matched raw samples and the integrity check passed.
   Missing Codex sample timestamps affected per-day attribution in that rehearsal (S-6); this is not acceptance of an installed hook upgrade.
   Not rehearsed: the in-place release build, the Codex `/hooks` approval, a hook firing during the binary replacement, and the console.
5. **Finish live console acceptance on isolated state.**
   A human must start the console in their own interactive terminal; do not bypass its agent-process guard.
   Navigate Trust & config and the affected plan views in a headed browser, interact with their primary controls on disposable fixtures, and confirm the resulting state at desktop and mobile widths.
   Record the tested commit, actions, screenshots and failures in the acceptance record.
   Use [console acceptance](console-acceptance.md) for the fixture scripts and the evidence template.
6. **Collect trial evidence, then complete release acceptance.**
   Review and trust policy in the human terminal, complete the observe-day and enforce-week trials for both agents, and record false denials, missed denials, recovery and interruption incidents using [trials.md](trials.md).
   Investigate each incident before expanding features.
   Then run the release workflow for all four targets, verify assets, provenance and notices as in [verifying releases](verifying-releases.md), complete clean-account install checks and compliance review, and test hosted updates when a published predecessor and successor exist, following [release acceptance](release-acceptance.md).
   Close #22 and reconcile #23 only with attached evidence, and keep unresolved limitations explicit rather than describing the system as defect-free.

## Safe console review with a database copy

Build the debug executable from the intended commit with `cargo build --locked -q -p yoke-cli`.
Then run the following from the repository root in your own terminal, with SQLite installed.
It copies the live database using SQLite's backup operation and lets the new console migrate only that temporary copy.
It does not replace the installed hook executable.
CLI commands, including the console, honour the isolated `HOME` in debug and optimized builds.
Optimized hooks (`yoke run-hook`) can use the account home instead; isolated hook rehearsals need a debug build or `--features honor-home-env`.

```sh
set -eu
umask 077
review_home=$(mktemp -d)
mkdir "$review_home/.yoke"
if [ -f "$HOME/.yoke/state.db" ]; then
  sqlite3 -readonly "$HOME/.yoke/state.db" ".backup '$review_home/.yoke/state.db'"
fi
printf 'Review state copy: %s\n' "$review_home"
HOME="$review_home" ./target/debug/yoke console --no-open
```

Keep that terminal open and use the printed one-time launch URL within five minutes.
The review home omits live agent settings and trusted configuration files, so installation and on-disk configuration diagnostics differ from the live account.
Use disposable project fixtures for any approval or recovery action: copying the database does not isolate project files or shell checks referenced by it.
Stop the console with Ctrl-C after review.

## Known limits to carry forward

- Yoke's self-protection keeps agents from accidentally or casually unblocking themselves; it is not a sandbox.
  The guard reads command words and the human-only commands look for agent markers, so a same-user agent that removes the markers, builds a path in pieces or uses its own tools can still reach the hook files, the database and the executable.
  Only operating-system isolation closes that.
- Gates and approved plan checks still run with the user's privileges outside the agent's sandbox.
  Approval now binds command text and declared input hashes; it does not sandbox checks or freeze undeclared code.
- When Yoke cannot read its state, a project last seen in observe mode is allowed through with a loud warning, and any other project is denied with the way back.
  A zero-byte `state.db` falls back to defaults with a warning, a payload over 1 MiB is denied in enforce mode, and a Stop blocked only because Yoke failed is let through the second time.
- Redaction is pattern based.
  A secret with no keyword or prefix is stored as typed, and rows stored before a redaction fix keep their original text.
- Agent memory commands use a hook-recorded session/process anchor and refuse missing or ambiguous scope; humans use their shell directory.
  Scope recovery is bounded to 128 candidate anchors and 128 recent decisions per candidate.
  Old pins still lack authorship, and trusted-only injection remains unimplemented.
- A hook ignores an injected `HOME` unless it is a debug build or was built with `--features honor-home-env`.
  Run an optimized build under an isolated home only as `CONTRIBUTING.md` describes.
- Transcript import counts at most the newest 32 MiB of a first read or of one append; a larger session shows as partly counted in `yoke usage`, and the usage in the skipped part is not counted.
  A Codex cumulative counter restart starts a new accounting segment, and a restart that lands exactly on the previous total is indistinguishable from no progress.
- A session silent for 30 minutes with pending facts is digested at the next session start in its project; a live session that silent is digested early, without its transcript.
- Ambiguous legacy notification ownership cannot always be recovered; hidden history must not be silently reassigned.
- Migration 013 can hold the write lock for seconds on a large history; migration 015 builds exact summaries once during upgrade.
  Hooks now queue durable notification catch-up rather than requiring hundreds of later starts; large-database upgrade latency has not been remeasured for this source.
- Stop-cap stalls still need real interruption-trial evidence, and no field test has yet observed a plan holding Stop in enforce mode in a live agent session.
- Binary artifact review has intentional limits; neither successful checks nor this handoff guarantee zero defects.
- A read-only command such as `yoke health`, `yoke usage` or `yoke statusline` on a database copy that lacks its `-wal` and `-shm` sidecars stops with a message that names `yoke log --limit 1`, and creates no file; run that command once to recreate them.
- A restored `state.db` beside newer `-wal` and `-shm` files silently replays their frames; restore or remove all three together.
- Codex skips hooks it has not approved without any message, and Yoke can only see whether an approval record exists, not whether it is current.
  After `yoke uninstall` and `yoke install`, the approval key of Yoke's hook survives only where Yoke's group is last in an event's list.
- Native Linux acceptance remains unverified for the failure-mode/layout cases, full ext4 disk, non-UTF-8 project names and interactive `yoke watch`.
  Gates now receive bounded marked input; hosted Linux CI is blocked by the reviewer fixture above.

Next session confirms hosted CI, reconciles the partial items and tests the new installed build with real agents, scheduling human-only acceptance alongside.
