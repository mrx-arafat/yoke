# Development handoff

Recorded 2026-10-17 at commit `0afac36`, after the second wave of fixes and features reached `main`.
This is the starting point for the next development session, not a release acceptance claim.
Track remaining acceptance in [issue #22](https://github.com/mrx-arafat/yoke/issues/22) and the [audit tracker #23](https://github.com/mrx-arafat/yoke/issues/23).

## State of main

`main` is at `0afac36`.
[CI run 39051349406](https://github.com/mrx-arafat/yoke/actions/runs/39051349406) and [installer run 39051349580](https://github.com/mrx-arafat/yoke/actions/runs/39051349580) passed for it.
No binary release is published, and nothing here replaced installed hooks.
The state schema is now 14, so a machine that already runs older hooks needs the coordinated upgrade below.

Commits since `746cef9`, the state recorded after the first field test, newest first:

- `0afac36` docs: document the security model, recovery paths, scale numbers and the observer
- `a42a20e` ci: harden the release and installer paths
- `6947257` fix: security hardening, failure-safe hooks, scale indexes and a richer observer
- `dea9c38` docs: record that enforcement blocks with real Claude Code and Codex
- `132f3b5` docs: record the first field test and refresh the development handoff
- `96112d8` chore: ignore the private .notes folder
- `8ce9b2d` feat: the installer gives Claude Code Yoke's statusline

The first-wave fixes are `250dd84`, `ca1cb8f` and `746cef9`; earlier commits back to `05310d7` are in `git log`.

## What is done

- **First field test.** 79 findings across seven journeys, all but one fixed: 78 are fixed and 1 is partly fixed (j4-3, the `usage_partial` notification).
  See the [field test record](field-testing.md).
- **Security.** A full audit found 29 findings; 10 are fixed, 8 partly fixed, 10 open and 1 declined.
  A session is pinned to the project it started in, gates run in the directory of the trusted `yoke.toml`, the guard protects the hook registration files and the executable, `YOKE_OFF` works only with the private token from `yoke pause`, `yoke install`, `uninstall` and `update` refuse agent processes, state is private, redaction covers more shapes, and injected memory is fenced and labelled as untrusted data.
- **Failure modes.** Hooks fail safely on malformed or oversized input, unreadable or locked state, killed processes and closed pipes.
  Observe mode keeps working when a project was last seen in observe mode and the state is unreadable.
- **Scale.** Migrations 013 and 014 add the indexes and the session anchors, and retention runs as a background job.
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

- **CI.** The run above passed for `0afac36`.
- **Installer matrix.** The installer run above passed for `0afac36`.
  At `0afac36` the workflow never passes `YOKE_COMMIT`, so its source builds are not the commit under test (release-1 in the ledger); the change that fixes it was uncommitted and not yet run when this record was written.
- **Live enforcement with real agents.** On 2026-10-17, on the field-test server, a policy in enforce mode with one gate blocked a marker command in Claude Code and in Codex after `yoke trust`, an ordinary command ran in both, and the store held a `deny` with the gate name for each agent.
  That run used the build from before the second wave.
  See [enforcement verified with real agents](field-testing.md#enforcement-verified-with-real-agents-2026-10-17).
- **Large-database timings.** Measured on one 16 GB laptop with a 740 MB fixture of 500 thousand decisions: every hook event stays at 5 to 10 ms in the median, and a Stop with a 5 MB transcript takes 35 to 60 ms.
  The numbers vary by tens of percent between runs and are not a guarantee.
  See [Scale](field-testing.md#scale).
- **Second-wave checks.** The audit's reproduction script flips from `VULNERABLE` to `SAFE` for SEC-1 to SEC-6 against a debug build.
  A kill storm of 360 hooks left the database intact, and 50 concurrent hooks all answered correctly.
  Every fix has focused tests, most written to fail first.
  See [second wave](field-testing.md#second-wave-2026-10-17).

Not verified: no real Claude Code or Codex session has run the second-wave build, the release workflow has not run, and the console has not been driven by a person.
The full list is under [Not verified live](field-testing.md#not-verified-live).

## What is open

- **Findings.** 23 ledger rows are open (7 P2 and 16 P3) and 11 are partly fixed; [open items](field-testing.md#open-items) groups them with a proposed fix for each.
  The open P2 items are the forged `yoke run-hook` events (SEC-7), the review prompt's delimiter and unredacted data (SEC-12 and SEC-13), records stranded by a renamed project (G-8), a non-UTF-8 project path (G-10), the uninstall guide's export command (out-3) and the installer workflow's source revision (release-1).
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
- `yoke notifications ack` and `yoke jobs retry` need a human terminal.
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

1. **Close the open P2 items.**
   `out-3` and `release-1` are small edits to `docs/uninstalling.md` and `installer.yml`, and the exact changes are in the [open items](field-testing.md#p2-open).
   SEC-7, SEC-12 and SEC-13 are code.
   G-8 and G-10 wait for the decisions in the previous section.
2. **Run the second wave with real agents.**
   A person at the keyboard, on a fresh server or an isolated account: `yoke trust` and enforce mode in both agents, a `cd` out of the project and a planted nested `yoke.toml`, `yoke pause` and the token, a plan holding Stop, Codex hook approval after a reinstall, the statusline styles in Claude Code, and `yoke watch` in a terminal pane.
   Record it as a new dated section in [field testing](field-testing.md), starting from the list under [Not verified live](field-testing.md#not-verified-live).
3. **Make the owner decisions and finish the partly fixed rows.**
   The remainders of SEC-8, SEC-10, SEC-14, SEC-19 and SEC-28 are the ones that change what the product promises.
4. **Coordinate the installed-binary upgrade.**
   Recheck which executable the installed hooks run and the state schema with `yoke health` before acting, then follow [upgrade notes](upgrade-notes.md): finish active checks, preserve a consistent database backup, replace every hook executable together, reinstall with the actual agent configuration directories, approve Codex hooks, and inspect `yoke health`.
   Do not open the real database with the new writable binary while old hooks still use it.
   An older binary against a newer database fails closed, which denies tool calls and blocks Stop even in observe mode; the installer script stops before replacing the executable in that case, so check `yoke health` yourself before a manual install.
   A rehearsal on 2026-10-15 with a debug build, an isolated `HOME` and an SQLite backup copy moved schema 6 to 12 in about 0.1 s, with the integrity and foreign key checks passing and rollback by restoring `state.db`, `state.db-wal` and `state.db-shm` together.
   Migration 013 was rehearsed on a clone of a 500 thousand decision database: about 9 s warm and 19 to 26 s cold, with a tested down script, and hooks that arrive meanwhile decide from read-only state without recording.
   Migration 014 is covered by the migration round-trip test only.
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
A debug build honours the isolated `HOME`; an optimized build ignores it unless it was built with `--features honor-home-env`, so do not substitute `target/release/yoke` here.

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
- Gates and approved plan checks run project code with the user's privileges, outside the agent's sandbox, and approval freezes the command text, not the code it runs.
- When Yoke cannot read its state, a project last seen in observe mode is allowed through with a loud warning, and any other project is denied with the way back.
  A zero-byte `state.db` falls back to defaults with a warning, a payload over 1 MiB is denied in enforce mode, and a Stop blocked only because Yoke failed is let through the second time.
- Redaction is pattern based.
  A secret with no keyword or prefix is stored as typed, and rows stored before a redaction fix keep their original text.
- The memory commands resolve the project from the shell's directory, because an agent shell exposes no verified session id.
  Pins that an agent made before the second wave cannot be told apart from a person's.
- A hook ignores an injected `HOME` unless it is a debug build or was built with `--features honor-home-env`.
  Run an optimized build under an isolated home only as `CONTRIBUTING.md` describes.
- Transcript import counts at most the newest 32 MiB of a first read or of one append; a larger session shows as partly counted in `yoke usage`, and the usage in the skipped part is not counted.
  A Codex cumulative counter restart starts a new accounting segment, and a restart that lands exactly on the previous total is indistinguishable from no progress.
- A session silent for 30 minutes with pending facts is digested at the next session start in its project; a live session that silent is digested early, without its transcript.
- Ambiguous legacy notification ownership cannot always be recovered; hidden history must not be silently reassigned.
- Migration 013 holds the write lock for seconds on a large history, and the first hooks after an upgrade read old history in windows, so a very large database takes many session starts to catch up.
- Stop-cap stalls still need real interruption-trial evidence, and no field test has yet observed a plan holding Stop in enforce mode in a live agent session.
- Binary artifact review has intentional limits; neither successful checks nor this handoff guarantee zero defects.
- A read-only command such as `yoke health`, `yoke usage` or `yoke statusline` on a database copy that lacks its `-wal` and `-shm` sidecars stops with a message that names `yoke log --limit 1`, and creates no file; run that command once to recreate them.
- A restored `state.db` beside newer `-wal` and `-shm` files silently replays their frames; restore or remove all three together.
- Codex skips hooks it has not approved without any message, and Yoke can only see whether an approval record exists, not whether it is current.
  After `yoke uninstall` and `yoke install`, the approval key of Yoke's hook survives only where Yoke's group is last in an event's list.
- Linux was not used for the failure-mode, layout and `yoke watch` passes, so the environment size limit for a gate, a full ext4 disk and a non-UTF-8 project path are reasoned, not observed.

Next session starts with the open P2 items, then runs the second-wave changes with real agents, with the steps that need a person scheduled alongside.
