# Development handoff

Recorded 2026-10-16 at commit `746cef9`, after the first field test and its fixes reached `main`.
This is the starting point for the next development session, not a release acceptance claim.
Track remaining acceptance in [issue #22](https://github.com/mrx-arafat/yoke/issues/22) and the [audit tracker #23](https://github.com/mrx-arafat/yoke/issues/23).

## State of main

`main` is at `746cef9`.
[CI run 38936025628](https://github.com/mrx-arafat/yoke/actions/runs/38936025628) and [installer run 38936025222](https://github.com/mrx-arafat/yoke/actions/runs/38936025222) passed for it.
No binary release is published, and nothing here replaced installed hooks.
The installer now adds Claude Code's statusline (`after-fix-1` in the [field test ledger](field-testing.md), fixed in 8ce9b2d).

Commits since `05310d7`, newest first:

- `746cef9` docs: document usage and quota, remote console, memory scope and the new guidance
- `ca1cb8f` fix: smooth the first-run, memory, plans, console and usage journeys
- `250dd84` fix: redact passwords in URLs and stop masking ordinary prose
- `bdee571` test: remove environment and timing hazards from the test suite
- `b01433f` docs: add the release runbook, gate status, uninstall guide and security policy
- `e2745ce` docs: draft the 0.1.0 release notes
- `6870a2f` ci: test the installer as a sudo user and through the interactive prompts
- `84e999e` release: add a read-only preflight for release tags
- `98e482b` ci: check Markdown links, anchors and dashes
- `689821a` build: make both crates packageable for crates.io
- `808aca8` feat: make the installer install everything on any common Linux and macOS
- `e51a32f` feat: install or upgrade Yoke with one curl command
- `a85b411` docs: add a one-line install and upgrade command
- `cd49834` test: assert the plan refusal names the existing plan's project
- `f1aa81b` docs: record release acceptance evidence for ff20d07
- `d16cb93` fix: explain the one-unfinished-plan-per-session rule when it refuses a plan
- `ff20d07` test: stop the guard allow-list depending on where the temp home lives
- `bbf094d` fix: keep CI green on current stable and with the truncated WAL
- `1695282` docs: record upgrade rehearsal, guard model, memory recovery and release evidence
- `8d0a6ab` test: pin review bases, session binding, health fields, first install, digest follow-up, orphan sweep and retention prune
- `463ba20` console: answer non-UTF-8 action bodies, escape badge classes, pin the HTTP surface; add acceptance kit
- `b0fc5e0` cli: describe every argument in --help, unify the tagline, reword the release check error
- `9e4cc8b` fix: digest facts of killed sessions and complete digest jobs atomically; cut hook latency
- `020e4ae` fix: start a new accounting segment when a Codex token counter restarts
- `f371bcb` fix: harden the self-protection guard against disguised human-only commands
- `088427b` ci: run formatting, clippy, tests, deny and changelog checks on every push and PR

## What is done

- **Hardening.** The self-protection guard resists disguised human-only commands, a Codex token-counter restart starts a new accounting segment, digests of killed sessions complete atomically, and the console HTTP surface is pinned with an acceptance kit.
- **Field test fixes.** The first [field test](field-testing.md) found 79 issues across seven journeys; 68 are fixed, 5 partly fixed and 6 open, and 250dd84, ca1cb8f and 746cef9 carry the fixes.
  They include `yoke usage` with Claude Code and Codex quota, a statusline quota segment, project-scoped memory ids, URL password redaction, `yoke console --port`, and an installer that refuses a state database newer than the executable.
- **CI.** Every push to `main` and every pull request runs formatting, clippy, the full test suite on Ubuntu and macOS, `cargo deny`, the changelog check and the documentation check for links, anchors and dashes.
- **Installer matrix.** The installer workflow runs the one-line installer on nine Linux images as root, as a user with sudo, and in a terminal where a driver answers the questions.
  No workflow runs the installer on macOS.
- **Release tooling.** Pushing a tag runs `release.yml`, which builds four targets on native runners and creates a draft release; `scripts/release/` packages and verifies the assets, and a read-only preflight checks a tag.
  The [release runbook](releasing.md), the draft [0.1.0 release notes](release-notes-0.1.0.md), the uninstall guide and the security policy are written.

## What is open

- **Field test.** The open and partly fixed findings are listed by priority in the [field test open items](field-testing.md#open-items), with a proposed fix for each.
- **Release gates.** The [gate status table](release-acceptance.md#gate-status) lists what is done, pending and human-only; it was last checked at `e2745ce`, so recheck it before relying on it.
  The release workflow has not run on all four targets, no release is published, and the compliance ticks, trademark search, observe-day and enforce-week trials, console acceptance and part of the install acceptance need a person.
- **Installed hooks.** A machine that already runs older hooks needs the coordinated upgrade below.

## Human-only steps

An agent cannot complete these and must not bypass their guards; record the exact command and stop.

- `yoke trust` and `yoke trust --global` approve a policy.
- `yoke console` starts the console.
- `yoke plan approve`, `resolve`, `acknowledge`, `resume`, `accept`, `cancel` and `reassign` decide a plan, and `yoke plan task done` does so for tasks owned by a human.
- `yoke notifications ack` and `yoke jobs retry` need a human terminal.
- Approving the Yoke hooks in Codex with `/hooks`.
- Publishing a draft release, ticking [COMPLIANCE.md](../COMPLIANCE.md), the trademark search, and the observe-day and enforce-week trials.

## Next work, in order

1. **Commit and verify the installer statusline change.**
   Check that CI and the installer matrix pass, then install on a fresh server and confirm that Claude Code shows Yoke's line and that an existing statusline is kept.
2. **Close the P2 field test items and the owner questions.**
   These are the `yoke log` scope, the notification links, and what remains of j4-3, j1-4, j3-memory-6 and j6-plans-console-5, plus the questions under "Decisions and checks that need a person" in [field testing](field-testing.md#open-items).
3. **Finish live console acceptance on isolated state.**
   A human must start the console in their own interactive terminal; do not bypass its agent-process guard.
   Navigate Trust & config and the affected plan views in a headed browser, interact with their primary controls on disposable fixtures, and confirm the resulting state at desktop and mobile widths.
   Record the tested commit, actions, screenshots and failures in the acceptance record.
   Use [console acceptance](console-acceptance.md) for the fixture scripts and the evidence template.
4. **Repeat the field test steps that need a person.**
   Cover `yoke trust`, `yoke plan approve` with a live Stop hold in enforce mode, the console through an SSH tunnel, and Codex hook approval after a reinstall, and record them as a new dated section in [field testing](field-testing.md).
5. **Coordinate the installed-binary upgrade.**
   Recheck which executable the installed hooks run and the state schema with `yoke health` before acting, then follow [upgrade notes](upgrade-notes.md): finish active checks, preserve a consistent database backup, replace every hook executable together, reinstall with the actual agent configuration directories, approve Codex hooks, and inspect `yoke health`.
   Do not open the real database with the new writable binary while old hooks still use it.
   An older binary against a newer database fails closed, which denies tool calls and blocks Stop even in observe mode; the installer script stops before replacing the executable in that case, so check `yoke health` yourself before a manual install.
   A rehearsal on 2026-10-15 with a debug build, an isolated `HOME` and an SQLite backup copy moved schema 6 to 12 in about 0.1 s, with the integrity and foreign key checks passing and rollback by restoring `state.db`, `state.db-wal` and `state.db-shm` together.
   Not rehearsed: the in-place release build, the Codex `/hooks` approval, a hook firing during the binary replacement, and the console.
6. **Collect trial evidence, then complete release acceptance.**
   Review and trust policy in the human terminal, complete the observe-day and enforce-week trials for both agents, and record false denials, missed denials, recovery and interruption incidents using [trials.md](trials.md).
   Investigate each incident before expanding features.
   Then run the release workflow for all four targets, verify assets and notices, complete clean-account install checks and compliance review, and test hosted updates when a published predecessor and successor exist, following [release acceptance](release-acceptance.md).
   Close #22 and reconcile #23 only with attached evidence, and keep unresolved limitations explicit rather than describing the system as defect-free.

## Safe console review with a database copy

Build the debug executable from the intended commit with `cargo build --locked -q -p yoke-cli`.
Then run the following from the repository root in your own terminal, with SQLite installed.
It copies the live database using SQLite's backup operation and lets the new console migrate only that temporary copy.
It does not replace the installed hook executable.

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

- Transcript import counts at most the newest 32 MiB of a transcript; a larger session shows as partly counted in `yoke usage`, and the usage in its older part is not counted.
  A Codex cumulative counter restart starts a new accounting segment, and a restart that lands exactly on the previous total is indistinguishable from no progress.
- A session silent for 30 minutes with pending facts is digested at the next session start in its project; a live session that silent is digested early, without its transcript.
- Ambiguous legacy notification ownership cannot always be recovered; hidden history must not be silently reassigned.
- Stop-cap stalls still need real interruption-trial evidence, and no field test has yet observed a plan holding Stop in enforce mode in a live agent session.
- Binary artifact review has intentional limits; neither successful checks nor this handoff guarantee zero defects.
- Read-only commands such as `yoke health`, `yoke trial report`, `yoke usage` and `yoke statusline` fail with SQLite error 14 on a database copy that lacks its `-wal` and `-shm` sidecars until a writer creates them; the live database has them.
- A restored `state.db` beside newer `-wal` and `-shm` files silently replays their frames; restore or remove all three together.
- Codex skips hooks it has not approved without any message, and Yoke can only see whether an approval record exists, not whether it is current.
- Rows stored before the redaction fix keep their original text, and `yoke log` still shows every project unless filtered.

Next session starts with the installer statusline change, then closes the P2 field test items, with the steps that need a person scheduled alongside.
