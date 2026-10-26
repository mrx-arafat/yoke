# Development handoff

Recorded 2026-10-26 at pushed commit `dca970c`, after round 2, wave 3 and the merged license-policy correction reached `main`.
This is the starting point for the next development session, not a release acceptance claim.
Track remaining acceptance in [issue #22](https://github.com/mrx-arafat/yoke/issues/22) and the [audit tracker #23](https://github.com/mrx-arafat/yoke/issues/23).

## State of main

`main` is at `dca970c`, following wave commit `15f613f`, `20b8801`, `d7ee984` and `2e8a549`.
The orchestrator's combined local gate passed 47/47 checks in the default temporary layout and `TMPDIR=/tmp`.
[Installer run 40275283119](https://github.com/mrx-arafat/yoke/actions/runs/40275283119) passed all 11 jobs.
[CI run 40275283127](https://github.com/mrx-arafat/yoke/actions/runs/40275283127) passed both Linux/macOS test jobs, formatting, clippy and docs/scripts, but its deny job failed: the current graph needs foldhash 0.2.0's Zlib license, removed from the older policy as unused.
The explicit Zlib allowance is restored in `dca970c`, with the unused BSD-2-Clause allowance removed.
[CI run 40275918992](https://github.com/mrx-arafat/yoke/actions/runs/40275918992) passed all six jobs, including deny and both Linux/macOS test jobs; W3-CI is fixed with hosted evidence.
The earlier macOS stale-policy test failure is repaired in `20b8801`; older FIFO/statusline CI failures are historical, not the current pending-work list.
Source supports schema 17, and the Linux field-test installation was upgraded to `091ffde`, schema 17, before the later wave was merged.
No published binary release or final-wave live acceptance is established by this checkpoint.
Upgrade all hook executables together and preserve a consistent database/sidecar backup; older writers refuse schema 17 and there is no supported downgrade command.

The current delivery commits are:

- `20b8801`: compare canonical paths in the macOS health fixture.
- `d7ee984`: redact gate failure reasons before truncation, restore log-row assertions, harden quota locking and improve recovery messages.
- `2e8a549`: distinguish CLI and hook HOME behavior and correct command examples.
- `15f613f`: integrate console origin checks, SHA256SUMS updater/installer verification, bounded Codex timestamp recovery, release/toolchain policy, schema-17 notes and Codex command previews.
- `dca970c`: allow the current graph's Zlib dependency and remove its unused BSD-2-Clause allowance.

Round 2 is `5e86630..091ffde`; earlier delivery and field results remain in [field testing](field-testing.md).

## What is done

- **Trust and failures.** Session/project anchors, trusted-policy gate cwd, self-protection, human-only mutations and fail-safe hook behavior remain in place.
  The new gate-diagnostic fix sanitizes before truncating, preserving supported credential labels until redaction; the original persisted-secret reproduction and controls pass.
- **Usage and state.** Source supports schema 17, exact usage summaries, quota history/headroom and historical policy versions on new decisions.
  Bounded replay recovers matching legacy Codex dates without deleting samples; child-rollout accounting remains a separate open defect.
- **CLI and observer.** Aligned tables and JSON consistency, concrete installer/reviewer recovery guidance, Codex command previews and watch cleanup are implemented.
  The console applies exact Host/Origin and request limits before launch redemption and authenticated reads, retaining action CSRF checks.
- **Installer and release tooling.** Downloads require exactly one matching SHA256SUMS entry before execution/replacement; invalid files preserve the old executable.
  Rust is pinned consistently for repository CI and release builds, dependency policy is explicit, and provenance remains a manual check.
- **Earlier findings.** The original journey fixes remain in the [field record](field-testing.md); this update adds four repaired round-2 audit findings and retains incomplete acceptance and audit coverage separately.

## What is verified

This is implementation evidence, kept apart from release acceptance.

- **Combined gate.** The orchestrator recorded 47/47 local checks before pushing; Installer passed all 11 jobs at `15f613f`, and all six hosted CI jobs passed at `dca970c` after correcting the license policy.
- **Focused behavior.** Independent console 28/28, updater 11/11, three loopback installer tests and the raw-output PTY regression pass.
  Later preview and installer pre-execution changes have their own RED/GREEN evidence; the gate-reason and log-row fixes pass both-layout focused targets and the original runtime reproduction.
- **Usage recovery.** A real debug Stop hook on schema 17 restored 1,702 dates, preserved all 3,331 raw samples and matched independently derived day/CLI totals.
  This does not claim that closed or unrecoverable real history has been repaired.
- **Linux field journeys.** At `091ffde`, live migration 14-to-17 passed, both real agents retained the original anchor after same-session resume from another cwd, watch filter/refresh/quit passed in a PTY and six real-callback statusline cases fit their widths.
  Continuous-process cwd changes and quota-present callbacks remain untested.
- **Corrected scale timing.** On a 704.5 MB fixture at optimized `20b8801`, each of 32 event/agent/mode cases has a latest p50 below 10 ms and p99 below 50 ms: ranges 5.646-8.267 ms and 7.060-19.639 ms.
  The first corrected matrix passed 29/32 and the three misses passed focused repeats; it was not one uninterrupted green run.
  The earlier all-case failure timed a new guard shell per sample and is superseded; Linux, transcripts, gates, backlogs and optimized timing of the later integrated wave remain outside this evidence.

See the [current wave record](field-testing.md#round-2-and-wave-3-verification-2026-10-26) and [remaining live gaps](field-testing.md#not-verified-live).
No human browser/console acceptance, published four-platform release, provenance verification or sustained-trial acceptance is claimed.

## What is open

- **Accounting and audits.** The 162-row ledger has 152 fixed, 5 partly fixed, 3 open and 2 declined.
  Child-rollout accounting remains open; the separate license-policy regression is fixed with green hosted CI.
  Timestamp recovery S-6 is fixed in source; child-rollout accounting is tracked as W3-5/W3-9.
  Both exhaustive W3-7 audits stay open: 349 of 1,226 command occurrences executed via 76 argv, 877 unexecuted; 66 of 394 error sites reviewed in context, 328 lexical-only.
  The 58 help paths passed, but that does not prove shell examples or human/prerequisite workflows.
- **Partial policy/security work.** SEC-8, SEC-10, SEC-23, SEC-28 and C13 retain their exact limits in [open items](field-testing.md#open-items).
  SEC-16 documentation is reconciled; SEC-23 candidate/stage checks are implemented but do not close replacement races or mutable-directory trust.
- **Decisions.** The owner questions are under [Decisions and checks that need a person](field-testing.md#decisions-and-checks-that-need-a-person): the worktree memory identity, the linked-worktree trust rule, injection of notes by default, managed-settings install and the statusline priority.
- **Release gates.** The [gate status table](release-acceptance.md#gate-status) was last checked at `e2745ce` on 2026-10-16, and `release.yml` has changed since, so recheck it before relying on it.
  The release workflow has not run on all four targets, no release is published, and the compliance ticks, trademark search, observe-day and enforce-week trials, console acceptance and part of the install acceptance need a person.
- **Installed hooks.** A machine that already runs older hooks needs the coordinated upgrade below.
- **Audit scope.** New reports replace the earlier missing summaries, but the exhaustive audits remain incomplete and the historical panic inventory is not a proof of the current whole program; see [free-model work](field-testing.md#free-model-work).

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
- Checking/enabling private vulnerability reporting, which `SECURITY.md` relies on and whose current setting was not reverified here: in the repository settings, or with `gh api -X PUT repos/mrx-arafat/yoke/private-vulnerability-reporting`.
- Creating the protected `release` environment with required reviewers and protecting `v*` tags, which `release.yml` expects before it publishes a draft.
- Ticking [COMPLIANCE.md](../COMPLIANCE.md) and doing the trademark search.
- Running the observe-day and enforce-week trials for both agents, and the console acceptance pass.
- Publishing a draft release.

## Next work, in order

1. **Repair child usage and validate its final source.**
   Current hosted CI is green at `dca970c`; retain that result separately from acceptance of subsequent accounting changes.
   Complete W3-9's regression and independent parent/child stream accounting without changing policy anchors, losing sibling samples or double-counting copied history.
   Reimport must be idempotent and unrelated transcripts must still be refused.
   Record focused, hosted, live and optimized evidence for the final integrated binary where needed, without rerunning unrelated successful checks.
2. **Finish the partial audits and policy decisions.**
   Continue W3-7 from its occurrence/site coverage ledgers; inventory and help success are not exhaustive command or error acceptance.
   SEC-8 still lacks trusted-only injection and memory authorship; SEC-10/SEC-28 retain manual provenance, signing, repository protection and maintainer automation decisions.
   Keep SEC-23 replacement-race limits and C13's zero-byte-state fallback explicit.
3. **Coordinate further upgrades and remaining live journeys.**
   Follow [upgrade notes](upgrade-notes.md): finish active checks, preserve a consistent database/sidecar backup, replace all hook executables together, use the actual agent config directories, approve Codex hooks and inspect `yoke health`.
   Live schema 14-to-17 migration passed at `091ffde`; rollback to an older binary against schema 17 is not supported or established by that run.
   Test continuous-process cwd changes, pause-token behavior, settings reload, an enforced plan holding Stop, reinstall approval and quota-present statuslines on safe state.
   Record actual revision and outcomes in [field testing](field-testing.md), keeping the completed resume/cwd, PTY watch and quota-absent callback checks distinct.
4. **Finish live console acceptance on isolated state.**
   A human must start the console in their own interactive terminal; do not bypass its agent-process guard.
   Navigate Trust & config and the affected plan views in a headed browser, interact with their primary controls on disposable fixtures, and confirm the resulting state at desktop and mobile widths.
   Record the tested commit, actions, screenshots and failures in the acceptance record.
   Use [console acceptance](console-acceptance.md) for the fixture scripts and the evidence template.
5. **Collect trial evidence, then complete release acceptance.**
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
- An optimized hook can ignore an injected `HOME` when the different account home already holds Yoke state, unless built with `--features honor-home-env`.
  Normal CLI commands honour `HOME` in every build; use `CONTRIBUTING.md` for isolated hook tests and a disposable account/container for shipped-hook acceptance.
- Transcript import counts at most the newest 32 MiB of a first read or of one append; a larger session shows as partly counted in `yoke usage`, and the usage in the skipped part is not counted.
  A Codex cumulative counter restart starts a new accounting segment, and a restart that lands exactly on the previous total is indistinguishable from no progress.
  Bounded timestamp replay repairs matching imported NULL dates rather than reconstructing all skipped usage; parent-hook/child-rollout accounting remains open.
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
- Native Linux acceptance remains incomplete for the failure-mode/layout cases, full ext4 disk and non-UTF-8 project names.
  Automated Linux PTY watch interaction and hosted Linux/macOS tests pass at their recorded revisions; complete six-job CI is green at `dca970c`.

Next work starts with child-rollout accounting and the pushed hosted checks, then continues the two exhaustive audits and remaining human/release acceptance.
