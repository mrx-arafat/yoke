# Field testing

[Documentation index](README.md) · [Release acceptance](release-acceptance.md) · [Development handoff](next-steps.md)

A field test walks Yoke the way a new user meets it: a fresh machine, the one-line installer, real Claude Code and Codex sessions, and every journey from first run to uninstall.
It differs from CI, which proves that code and the installer behave on clean images, and from [release acceptance](release-acceptance.md), which gates a published release on evidence that a person must supply.
A field test does not pass or fail.
It produces findings about the experience, such as wrong guidance, silent failures and misleading documentation, and this page keeps them.

This page records what was tested, how Yoke behaves in real use, the second-wave audit and failure-mode passes, every finding with its status, and how to repeat the test.
It is a record of observations, not a release acceptance claim.

## Implementation verification 2026-10-18

The current source is `9df0602`, following code commit `6a98f22` and CI/installer commit `a2b3428`.
Final local verification passed all 41 checks, including formatting, clippy, script checks and Rust targets in both the default temporary layout and `TMPDIR=/tmp`.
The six integration fixture failures were reconciled against the new security contracts before that final pass.
[Installer run 39058112818](https://github.com/mrx-arafat/yoke/actions/runs/39058112818) passed all 11 jobs at `9df0602`.
[CI run 39058112836](https://github.com/mrx-arafat/yoke/actions/runs/39058112836) passed formatting, clippy, docs/scripts and dependency checks, but failed the test jobs on both macOS and Ubuntu.
Both failures are `digest_rejects_fifo_without_waiting_for_a_writer`: the fixture reaches absolute reviewer lookup without a stubbed `claude` executable, so it stops before the intended regular-file refusal.
The reported local fix validates the bounded regular transcript before resolving the reviewer and checks the FIFO case with an empty PATH; a new commit and green hosted CI are still required.
The local 41-check pass therefore does not establish hosted CI acceptance.
Installed real-agent evidence below remains at `d5d4ed5`, schema 14, while the new source supports schema 15.
The orchestrator rehearsed migration 015 on a schema-14 state copy: notifications were retained, session totals matched raw samples and the integrity check passed.
That rehearsal also found Codex usage samples without timestamps, which are excluded from per-day totals (S-6); timestamp recovery and backfill remain open.

New benchmark fixtures stayed below 50 MB.
A 20,000-decision background drain completed in 261 ms with exact-count checks, and final bounded function measurements were p50 1.229/1.368 ms and p99 1.610/1.834 ms without/with durable queue work.
These are separate from complete debug-process hook medians of 16.7 to 17.8 ms, which miss the p50 below 10 ms target.
The 700 MB target and optimized-release latency remain unverified in this wave; the older scale measurements below are historical evidence.

## Field test 2026-10-17: current installer and real agents

The documented one-line installer built `d5d4ed5`, matching the current `main` revision before and after installation.
The executable reported `0.1.0`, and health reported state schema 14 with supported schema 14.
This was a Linux field test of that installed revision; it does not accept the newer implementation commits or a hosted release.

Twelve sequential real-agent sessions covered ordinary commands, trusted-gate denials, writes to protected hook settings and `YOKE_OFF=1` without a pause token.
Both agents allowed ordinary commands, denied the marker command under the named gate, and denied all four attempted settings writes through self-protection.
Protected settings and the trusted policy retained their initial SHA-256 values.
An administrator independently queried the decision database read-only and confirmed all twelve expected session outcomes.

The two directory-change attempts did not change the cwd reported in the hook payload.
Their denials and stored session anchors therefore do not establish enforcement after a real cwd transition; that live acceptance criterion remains open.
No extra agent sessions were added beyond the twelve-call budget.

Detailed statusline output fit widths 60, 100 and 160, with observed line widths of 59/56, 100/56 and 100/56 terminal cells.
The non-terminal watch printed one snapshot and exited; usage, project-scoped log and explicit all-project log also completed.
These single CLI timings do not establish hook latency percentiles or a large-database performance target.

The installed revision still repeats the Codex review warning after a no-op install (G-11), and partial transcript accounting remains visible (j4-3).
The test also found unsafe release-acceptance isolation guidance: changing HOME alone does not isolate a shipped optimized executable (release-2, corrected in `9df0602`).
Use a disposable operating-system account or container for shipped artifacts; debug and `honor-home-env` builds establish source rehearsal only.

## Field test 2026-10-16

### Environment

- A fresh Ubuntu 24.04 x86_64 server, a non-root user without passwordless sudo, Claude Code and Codex installed and logged in.
- Yoke 0.1.0, installed by the one-line installer from `main`.
  No release was published, so the installer built it from source.
- At the start: state schema 12, observe mode, no trusted policy, hooks connected for both agents.
- Findings were recorded against commit `bdee571`, the last commit before the fixes.

### Method

Automated agents walked each journey as a new user would, one agent per journey, and each wrote a report.
A report holds the commands run with their real output, every finding with steps, expected and actual results and a proposed fix, notes on what the user gains, and the steps that only a human may run.
The agents did not run human-only commands (`yoke trust`, `yoke console`, `yoke plan approve`), so those paths were derived from source and tests and are marked as such below.
Probes that install, uninstall or migrate ran in throwaway home directories that were deleted afterwards, and the real state was only read.
The finding IDs keep the journey labels used during the test (`j1` to `j8`); this record has no journey 5.

| Journey | What it covered | Findings |
| --- | --- | --- |
| j1 first run | A fresh login: `yoke`, `health`, `init`, `gates`, `config`, `log`, notifications, a first plan, a first Claude Code and Codex run, exit codes and error messages | 14 |
| j2 live agent sessions | Interactive Claude Code and Codex in a demo project, approving the Codex hooks, the statusline | 8 |
| j3 memory | `remember`, `recall`, pin, share, forget, digest, export and import, redaction, a real Claude Code session | 14 |
| j4 usage and notifications | Recorded usage against each agent's own totals, the statusline input, notifications, the error log, the release check, rate limits in both agents' data | 10 |
| j6 plans and console | A plan from scaffold to validation and review, refused transitions, the Stop hold and the console on a remote server (from source) | 11 |
| j7 extensions and Codex | The extension inventory and drift, stale hooks, Codex hook events, Codex hook approval, the Codex sandbox | 12 |
| j8 install lifecycle | Fresh install, rerun, a moved executable, an old and a newer state database, uninstall, reinstall | 10 |

The test produced 79 findings: 7 P1, 32 P2, 39 P3 and 1 note, using the severities the journey reports assigned.
Four more observations came from the first check of the fixed build.
After the first wave of fixes, 68 findings were fixed, 5 partly fixed and 6 open.
The [second wave](#second-wave-2026-10-17) closed 10 of those 11, so 78 are fixed and 1 is partly fixed (j4-3).

### What was verified after the fixes

- The fixes were made in one wave and committed as `250dd84`, `ca1cb8f` and `746cef9`.
  Each fix was checked by focused tests, and most also by running a debug build in a throwaway home directory on the development machine.
  The plan and console fixes were checked by tests only.
  The usage fix used real captures from the test server as input: a Claude Code statusline callback, a Claude Code transcript and a Codex session record.
- CI (formatting, clippy, tests on Ubuntu and macOS, dependency and documentation checks) and the installer matrix passed for `746cef9`: [CI run 38936025628](https://github.com/mrx-arafat/yoke/actions/runs/38936025628) and [installer run 38936025222](https://github.com/mrx-arafat/yoke/actions/runs/38936025222).
- `yoke update --check` against the real release endpoint, in a throwaway home, printed `latest_version: none` and exited 0 while no release was published.
- On the test server, two observations followed.
  A user saw no Yoke line in Claude Code because the installer never set Claude Code's `statusLine`; it was added by hand with `yoke install --agent claude --statusline`, and the installer now adds it (fixed in 8ce9b2d).
  `yoke usage` showed only a weekly Codex window because Codex itself reported only a 7-day `primary` window and `secondary: null` on that plan, so Yoke was correct.

Not verified live after the first wave (the current list is under [Not verified live](#not-verified-live) in the open items):

- The console in a browser, on a server through an SSH tunnel, and the new wrong-address page.
- A plan holding Stop in enforce mode in a real agent session.
  The `Stop` behaviour comes from source and tests.
- Whether Codex keeps its hook approval after `yoke uninstall` and `yoke install`.
- The installer refusing a database written by a newer Yoke, with a real newer build.
  It was tested with an edited schema version and a stub executable.
- An upgrade from one published release to a newer one, because only one version exists.
- The macOS installer path and every step that needs `yoke trust` in a human terminal.

## How Yoke behaves in real use

These are observations from the test server and the throwaway homes.
Behaviour described as "after the fixes" was checked in throwaway homes and by tests, not on the test server.
Items marked "from source" were not observed in a live session.

### Install and first run

- The one-line installer needs no root when a C compiler is present.
  It puts `yoke` in `~/.local/bin`, connects the hooks of each agent it finds, creates the state database at the current schema and adds one PATH line to the shell startup files.
- Rerunning the installer changes nothing: the agent settings, receipts and PATH lines stay byte for byte the same.
- A fresh install reports `health: attention` with exit 0 until a human trusts a policy and a hook event has been recorded.
  `attention` means setup is incomplete, not that something is broken.
- `yoke init` writes a starter `yoke.toml` that contributes no gates until a human runs `yoke trust`.
- The PATH line sits in interactive shell startup files, so cron, scripts and `ssh host command` need the full path to the executable.

### Live agent sessions

- Hooks fire for every Claude Code and Codex event: session start, prompt submit, pre and post tool use, stop and session end, plus notifications for Claude Code.
  Both agents' events appear in `yoke log`.
- Codex asks once to review the hooks ("7 hooks need review").
  Opening `/hooks` lists each Yoke hook with its exact command, and pressing `t` trusts them all.
- Until the Codex hooks are approved, Codex skips them silently, including in `codex exec`, and `yoke log` gains nothing.
- Claude Code needs no separate approval for Yoke's hooks beyond its own prompt to trust the project folder.
- Observe mode records every event and never blocks.
  It writes nothing to the hook's standard output, so the agent sees no difference.
- A hook costs 7 to 24 ms per event on the test server.
- A global install records every Claude Code and Codex session of that user, in every project.
- Gates run only after a human trusts a policy.
  Until then even the starter `no-force-push` gate records nothing on a force push.
- When Yoke cannot read or write its state, hooks fail closed: they deny tool calls and block Stop even in observe mode.
  This was seen with a state database newer than the executable.

### Memory

- Notes belong to a project: the Git root, or the exact directory outside Git.
  After the fixes, `forget`, `share` and `memory pin` refuse an id from another project.
- Recognizable secrets are masked before a note is stored.
  After the fixes that includes passwords in URLs and the whole value after `key: `, and ordinary prose next to the word "token" or "password" stays readable.
  Rows stored by earlier builds keep their original text.
- Session start injects notes only in enforce mode.
  In observe mode an agent that was asked about a pinned note answered that it had none; `recall --explain` and `remember` now say so.
- Digests stay off until a human enables and trusts them, while facts keep accruing.
- An export is plain JSON with mode 0600, and an import needs one `--map SOURCE=DESTINATION` for each project in the archive.

### Usage and notifications

- Usage is imported at Stop, session end and before compaction.
  In three Claude Code runs the recorded cost and tokens matched the agent's own totals to the last digit, and Codex totals matched once cached input is counted separately.
- Codex samples carry no price by design, so Codex cost is shown as unknown.
- Both agents report rate limits: Claude Code in the statusline callback, Codex in its session records.
  A Codex plan can report one window only, so Yoke labels each window by its length instead of assuming five hours.
- A transcript larger than 32 MiB used to disappear from usage.
  Now its newest 32 MiB are counted and `yoke usage` marks the session as partly counted.
- The `config_untrusted` notification appears as soon as `yoke init` creates a file that is not trusted.
- The daily release check no longer logs an error while no release is published.

### Plans and console

- A plan needs an initial Git commit and a Git identity.
  Without a commit, `plan diff` used to fail with raw Git text and now explains the prerequisite.
- The independent reviewer returns blocking findings for the unedited scaffold in about 7 seconds, so the scaffold must be edited before approval.
- Refusals before approval name the plan's state and the next command.
  Plans hold Stop only in enforce mode (from source).
- The console listens on `127.0.0.1` only, on a random port unless `--port` is given.
  Its launch link works once within five minutes, and the `Host` header must be exactly `127.0.0.1:<port>` (from source).
- On a server, run `yoke console --no-open` and forward the same port number through SSH (from source).

### Extensions and Codex

- `yoke extensions` is read-only, deterministic and fast.
  It reports `shadowed`, `stale` and `dangling` definitions, and it never prints environment values, only their key names.
- A stale managed hook is repaired with `yoke uninstall`, then `yoke install`.
  `yoke install` refuses while a managed hook is edited, with or without `--dry-run`.
- Codex tool names reach `yoke log` as Codex spells them, such as `Bash`, `apply_patch` and MCP names.
  A failed `apply_patch` has no post-tool event because Codex has no failure hook.
- Codex's own sandbox can fail on Ubuntu 24.04 when unprivileged user namespaces are restricted.
  Commands then fail with `bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted` while Yoke records `allow`.

### Install lifecycle

- Moving the executable works through the installer, which removes and recreates the hooks for the new path.
  A manual `yoke install` from another path is refused with a message that names both paths.
- A schema 6 database migrates to 12 on the first writable command after the hooks use the new executable, and its rows survive.
  `yoke health` never migrates, and `yoke log` does.
- Uninstall returns the agent settings files byte for byte, removes files the installer created and keeps timestamped backups.
- `YOKE_AGENTS=none` leaves the state database alone, and the installer refuses a database newer than the executable before it touches agent settings.

## Enforcement verified with real agents (2026-10-17)

This is the check that matters most, because it is the product's core promise, and it was run live on the test server with both agents.

Setup: a throwaway project with a policy in enforce mode and one gate that denies any `Bash` command containing a harmless marker word.
The policy was reviewed in a terminal and trusted with `yoke trust`, which printed the exact file bytes and asked for a typed confirmation.

| Step | Claude Code | Codex |
| --- | --- | --- |
| Marker command with the policy untrusted | ran and printed the marker | ran and printed the marker |
| Same command after `yoke trust` | blocked before it ran; the agent reported the gate message and said it did not try to work around it | blocked by the hook; the agent reported the gate message and "no command output was produced" |
| An ordinary command after `yoke trust` | ran normally | ran normally |

What the store recorded: for each agent, one `deny` decision with the gate name and the blocked command at the moment of the marker command, and `allow` for the ordinary command and for each `stop`.
Claude Code's statusline changed to `yoke enforce | ! recent deny | ...`, so the block was visible without opening the log.
`yoke gates` listed the gate only after trust and showed no gates before it, as designed.

Takeaways for the manual:
- An untrusted policy does nothing, so a user who only runs `yoke init` is still in observe mode with no gates; the health guidance and statusline say so.
- Both agents surface the gate's message to the model, so write messages that tell the agent what to do instead.
- Trust is per exact file content: edit `yoke.toml` and the gate stops applying until it is trusted again.

## Second wave 2026-10-17

The first field test and its fixes were followed by four passes that attack Yoke from the inside, and by the fixes and features they led to.
They ran on the development machine, in isolated home directories with debug builds, not on a fresh server with real agent sessions.
Like the first test they produce findings, and none of this is a release acceptance claim.

Commit `dea9c38` was the base.
`6947257` carries the code, `a42a20e` the release and installer hardening and `0afac36` the documentation.
CI and the installer matrix passed for `0afac36`: [CI run 39051349406](https://github.com/mrx-arafat/yoke/actions/runs/39051349406) and [installer run 39051349580](https://github.com/mrx-arafat/yoke/actions/runs/39051349580).

### The four passes

| Pass | What it did | Result |
| --- | --- | --- |
| Security audit | A read-only review of the trust model, the guard, hooks, memory, review, the installer, the updater, the release workflow and the console changes since the earlier console audit; each finding was reproduced in an isolated home or marked as read from code or documentation | 29 findings (3 P1, 11 P2 and 15 P3): 10 fixed, 8 partly fixed, 10 open, 1 declined |
| Failure modes | 16 shell probes and 21 new focused tests against the real executable: malformed, oversized and truncated payloads, unreadable, locked, damaged, full or replaced state, killed hooks, closed pipes, concurrent hooks and gate floods | 17 defects (`C1` to `C17`): 16 fixed, 1 partly fixed |
| Project layouts | 17 layouts, among them subdirectories, linked worktrees, submodules, bare repositories, symlinks, renamed directories, monorepos with nested policies and a repository at the home directory, each with `init`, `gates`, `config`, hooks, plans, memory and `log` | 6 defects fixed, 4 more findings open (`G-1` to `G-11`) |
| Scale | A 740 MB fixture with 500 thousand decisions, timed per command and per hook before and after the fixes (see [Scale](#scale)) | 3 defects (`S-1` to `S-3`): 2 fixed, 1 partly fixed; 2 more findings open |

### What changed because of them

- A session is pinned to the project it started in, gates run in the directory of the trusted `yoke.toml`, and the guard protects the hook registration files and the executable.
- `YOKE_OFF` works only with a private token that the human-only `yoke pause` prints, `yoke install`, `uninstall` and `update` refuse agent processes, and `yoke health` and the statusline flag the ways to switch hooks off.
- State is private (0700 and 0600), redaction masks more credential shapes, the log keeps the size and status of a tool's output, injected memory is fenced and labelled as untrusted data, and a memory import is validated and previewed.
- Hooks fail safely.
  Observe mode keeps working when a project was last seen in observe mode and the state is unreadable, a Stop that was blocked only because Yoke failed is blocked once, and a closed stderr, a panic or a pipe in place of the database can no longer end a hook without an answer.
- Three indexes and tighter queries keep `yoke health`, `yoke trial report` and filtered `yoke log` fast on a large history, and the hourly retention pass runs as a background job.
- `yoke log` lists the current project by default, `yoke usage` resumes each transcript where the last import stopped, and the plan commands print text with `--json` for scripts.
- The statusline is a width-aware row of segments with `--style compact` and `--style detailed`, and `yoke watch` is a live, read-only terminal view of the current project.
- The state schema is now 14.
  An older executable fails closed against a database that a newer build has migrated; [upgrade notes](upgrade-notes.md) has the order of an upgrade.

### How it was checked

- The audit's reproduction script prints `VULNERABLE` or `SAFE` per line against any debug build.
  The lines for SEC-1 to SEC-6 flipped, the redaction battery leaks 4 of 39 shapes instead of 32 (three are generic secrets with no pattern and one is a probe artifact), and SEC-7 still reproduces.
- A kill storm of 6 rounds of 60 hooks, each killed at a random point in its first 24 ms, left the database intact: the integrity check passed, there were no foreign key violations or orphan rows, trust was unchanged and the next hook behaved normally.
  50 concurrent PreToolUse hooks and 50 concurrent SessionStart hooks all answered correctly and recorded one row per hook.
- Every fix was checked by focused tests, most of them failing first.
  The new test files are `anchor.rs`, `chaos.rs`, `layouts.rs`, `watch_cli.rs`, `data_hygiene.rs`, `state_faults.rs` and `scale_indexes.rs`.
- `yoke watch` was driven through a pseudo-terminal: keys, resize, Ctrl-C and SIGTERM left the terminal in its original state, and a 257 MB database took 0.06 to 0.15 s per warm snapshot in a debug build.
- No real Claude Code or Codex session ran against the second-wave build.
  What is not verified is listed under [Not verified live](#not-verified-live).

### A process finding

Two runs wrote into a maintainer's real state database although they were meant to use an isolated one: a test that removed `HOME` fell back to the account's home directory, and optimized builds ignored an injected `HOME`.
One of them migrated the real database.
The fix is `proc-1` in the ledger: an unset or relative `HOME` is an error in every build, only debug builds and builds with `--features honor-home-env` honour `HOME`, and `CONTRIBUTING.md` says how to run an optimized build under an isolated home.

## Scale

Yoke records every hook event, so the state database only grows.
This section records what that costs after months of heavy use.
The sizes are illustrative, not taken from a real installation.

### Method

- Fixture: an isolated home whose `state.db` was created by the product, then filled with generic rows by direct SQL.
  500 sessions, 500 thousand decisions (about 730 bytes each, payloads capped at 4 KiB), 200 thousand facts, 50 thousand usage samples, 5 thousand memories, 100 plans with 1.4 thousand check runs, 2 thousand jobs and 400 notifications.
  The database was 740 MB, of which decisions were 366 MB and facts 193 MB.
- Measurements: release builds, every command and hook run as a real process, medians of 5 runs per command and 40 per hook.
  The machine was a 16 GB laptop shared with other workloads (load average 4 to 10), so absolute numbers vary by tens of percent between runs.
  Read the ratios, not the digits.
- Thresholds: a hook should stay under 100 ms and an interactive command under 1 s.
- This is implementation evidence from one machine.
  It is not a performance guarantee and not release acceptance.

### Before and after

"Before" is the build before schema 13.
"After" is schema 13 together with the query changes that ship with it: filters that were not given are no longer part of the log query, and the trial report formats 15-minute slots instead of every decision.

| Command | Before | After |
| --- | --- | --- |
| `yoke health` | 280 ms (3 to 4 s on a cold cache) | 3 ms |
| `yoke trial report` | 1.9 to 3.6 s (6 s and more on a cold cache) | 0.4 s |
| `yoke log --tool <name that never ran>` | 180 to 610 ms | 4 ms |
| `yoke log --outcome error` | 31 ms | 4 ms |
| `yoke log --outcome would_deny` | 18 to 23 ms | 4 ms |
| `yoke log --session <id>`, `--project <path>` | 13 ms | 5 ms |
| `yoke log`, `yoke log --limit 1000` | 6 ms, 8 to 13 ms | 4 ms, 7 ms |
| `yoke notifications`, first runs after an upgrade | 270 to 370 ms per run, 124 runs to catch up | 430 ms per run, 13 runs to catch up |
| `yoke usage` | 41 ms with 50 thousand samples, 300 ms with 500 thousand | unchanged |
| `yoke recall`, `statusline`, `config`, `gates`, `plan list`, `jobs`, `extensions` | 3 to 12 ms | unchanged |

Hooks stay far below the threshold at this size.
Median per event: PreToolUse 4.8 ms, PostToolUse 5.7 ms, UserPromptSubmit 5.1 ms, SessionStart 6.3 ms, SessionEnd 6.9 ms, Stop 9.9 ms with a 300-line transcript.
The three new indexes add about 0.1 to 0.2 ms (about 3 percent) to each recorded event and 49 MB (6 percent) to the database.

### What grows and what is bounded

- Decisions, facts and finished check runs are deleted after `log.retention_days` (90 by default), so at the fixture's rate the database settles near 0.75 GB.
  The file does not shrink after a prune; the freed pages are reused.
  After pruning 150 thousand decisions and 39 thousand facts about a quarter of the pages were free.
- Sessions, memories, usage samples, plans and their transitions are never pruned.
  They are small: a usage sample is about 150 bytes, but `yoke usage` reads every sample, at about 6 microseconds each.
- The write-ahead log is empty after the last hook exits.

### Retention runs in the background

The hourly cleanup used to run inside the SessionStart hook.
The hook now only queues it as a background job, and a worker deletes in short batches with a 110 ms pause between them so that waiting hooks get the write lock.
Measured on a 289 MB fixture with 52 thousand decisions and 21 thousand facts past the boundary.
The optimized build cannot be pointed at an isolated home, so these runs used unoptimized builds; read the ratios, not the digits.

| | SessionStart that has the cleanup due | While the backlog drains |
| --- | --- | --- |
| Before: inside the hook | median 66 ms, p95 106 ms, max 457 ms, in each of 67 sessions (about 5 s of deleting inside hooks) | each of those sessions waited |
| After: background job | 26 ms; 15 to 20 ms once nothing is left to delete | the backlog was gone after 12.5 s; PreToolUse during the drain: median 17 ms, p95 60 ms, max 372 ms |

With nothing to delete, a SessionStart that has the cleanup due costs 16 ms before and 15 to 20 ms after.

### Migration 013

Migration 013 builds three indexes on an existing database.
On the fixture it took about 9 seconds with a warm cache and 19 to 26 seconds on a cold one, which is roughly one second per 50 thousand decisions.
A fresh installation pays nothing.
While it runs, the first hook holds the write lock.
Hooks that arrive meanwhile wait 2 seconds, then decide from read-only state: gates still apply and a deny is still a deny, but those events are not recorded.
See [upgrade notes](upgrade-notes.md) for how to run it on purpose.

### Known limits

These describe the historical scale run.
Commits `6a98f22` and `a2b3428` add durable notification catch-up and exact usage summaries; the newer bounded measurements above do not revalidate the historical large-database target.

- The first session starts after an upgrade from a build without notifications also read old history: each hook takes a window of 2,000 decision ids, the `yoke notifications` command 20,000.
  Draining 500 thousand decisions therefore takes about 250 session starts or 13 runs of the command.
  A window of 20,000 in the hook took 350 to 620 ms per hook (unoptimized build); 2,000 takes 40 to 120 ms there.
- Stop re-reads the whole transcript.
  It took 35 to 60 ms for a 5 MB transcript and grows with the file.
- `yoke usage` grows linearly with the number of samples, because usage is never pruned.
- Cold-cache first runs after a reboot are several times slower than the numbers above for any command that scans a table.

## Findings ledger

One row per finding, retaining its original severity and source location.
It holds 154 rows: 83 from the first field test (79 journey findings and 4 observations from the first check of the fixed build), 69 from the second wave, one release-rehearsal finding from the current Linux field test and one usage finding from the schema-15 rehearsal.
Of the 154, 145 are fixed, 6 are partly fixed, 1 is open and 2 are declined.
The six partly fixed rows are SEC-8, SEC-10, SEC-16, SEC-23, SEC-28 and C13; release acceptance remains separate from implementation status.
Status is `fixed in <short sha>`, `partly fixed in <short sha>` with what remains in brackets, `open` or `declined`.
The first wave is carried by `250dd84`, `ca1cb8f` and `746cef9`.
The second wave is carried by `6947257` (code), `a42a20e` (release and installer paths) and `0afac36` (documentation only).
The current implementation wave is carried by `6a98f22` (code), `a2b3428` (CI, benchmark and installer migration) and `9df0602` (documentation).
The ID prefix says where a row came from:

| Prefix | Source |
| --- | --- |
| `j1-` to `j8-` | the seven journeys of the first field test (there is no journey 5) |
| `after-fix-` | the first check of the fixed build |
| `SEC-1` to `SEC-29` | the second-wave security audit (3 P1, 11 P2 and 15 P3) |
| `C1` to `C17` | the failure-injection pass |
| `G-1` to `G-11` | the project layout pass (there is no `G-9`) |
| `S-1` to `S-5` | the large-database pass |
| `proc-`, `out-`, `anchor-`, `health-`, `release-` | found while fixing and testing the second wave |

The audit and layout reports gave their own severities.
The severities of `C1` to `C17`, `S-1` to `S-5`, `proc-1`, `anchor-1` and `release-1` were assigned when this ledger was written, from the effect each report describes.
Several findings share a cause, such as the log scope (j1-2 and j3-memory-14), the notification link (j1-7 and j4-5) and the project policy that followed the agent's directory (SEC-1, SEC-4 and G-1).

| ID | Severity | Area | Summary | Status | Where |
| --- | --- | --- | --- | --- | --- |
| S-6 | P2 | Usage | Codex usage samples with NULL timestamps are excluded from per-day totals, despite contributing to session totals | open (found during the schema-15 copy rehearsal; investigate rollout timestamps and backfill where recoverable after CI is green) | `crates/yoke-core/src/usage.rs`, `crates/yoke-core/src/store/015-usage-summaries.sql` |
| SEC-7 | P2 | Security | `yoke run-hook` accepts forged events from an agent shell and writes decisions, facts and the Stop counter | fixed in 6a98f22, 9df0602 (guard denies agent run-hook commands; records OS parent PID/name/start identity rather than trusting submitted provenance; same-user provenance is not cryptographic authentication) | `crates/yoke-cli/src/hook.rs`, `crates/yoke-core/src/guard.rs` |
| SEC-12 | P2 | Review | the author of a diff can imitate the review delimiter and spoof the independent verdict | fixed in 6a98f22, 9df0602 (independent review uses unpredictable framing and JSON encoding for diff data; delimiter injection regression covered) | `crates/yoke-core/src/review.rs` |
| SEC-13 | P2 | Review | review sends the unredacted diff and untracked files to the other vendor | fixed in 6a98f22, 9df0602 (review inputs redact known secret forms and skip secret-looking files; vendor/data disclosure documented; arbitrary unknown-secret detection is not guaranteed) | `crates/yoke-core/src/review/input.rs` |
| G-8 | P2 | Layouts | after a project directory is renamed, trust is lost as designed but memory, plans and log stay under the old path, and no supported command moves them | fixed in 6a98f22, 9df0602 (human-only project move supports dry-run and transactional relocation with conflict checks, rollback and byte-verified trust batches; physical directories/global trust are not moved) | `crates/yoke-core/src/store.rs` |
| G-10 | P2 | Layouts | on Linux a project directory whose name is not UTF-8 makes the policy unavailable and denies every tool call, even in observe mode (reasoned from code, not reproduced) | fixed in 6a98f22, 9df0602 (unrepresentable project scope retains trusted parent/global policy rather than denying every tool call; synthetic-byte regression passed, native Linux filename acceptance remains unverified) | `crates/yoke-core/src/trust.rs` |
| out-3 | P2 | Docs | the uninstall guide saves decisions with `yoke log --limit 1000 --json`, which now exports only the current project | fixed in 9df0602 (guide uses `--all` and explains current-project scope; export and isolated uninstall evidence checked) | `docs/uninstalling.md` |
| SEC-18 | P3 | Gates | on Codex a rewrite gate answers `allow` and skips Codex's approval prompt | fixed in 6a98f22, 9df0602 (Codex rewrite requires narrow tool and content matchers; documented explicit allow behavior and configuration regression preserved) | `crates/yoke-core/src/wire.rs` |
| SEC-20 | P3 | Repository | four tracked files embed a maintainer's account name and home path | fixed in 6a98f22, a2b3428, 9df0602 (portable and relative paths; tracked-tree scan clean; six bypass tests passed) | `scripts/acceptance/lib.sh`, `docs/specs/`, `crates/yoke-cli/src/console/artifacts/impeccable.json` |
| SEC-22 | P3 | Plans | `yoke plan approve` prints the header as one JSON string, and Unicode formatting characters could change what a person reads (terminal rendering not verified) | fixed in 6a98f22 (line-preserving escaped preview has focused coverage; human terminal acceptance remains unverified) | `crates/yoke-cli/src/workflow.rs` |
| SEC-23 | P3 | Installer | the hook command is the executable path as found, with no check that the file or its directories are writable by others | partly fixed in 6a98f22, 9df0602 (regular executable and canonical path required; unprotected world-write is a problem, group-write is guidance and protected sticky ancestors are accepted; shell pre-execution regular-file check remains a proposal) | `crates/yoke-cli/src/install.rs` |
| SEC-27 | P3 | Installer | installer and health robustness: the JSON re-render drops whitespace, the unchanged check runs before the write, the install lock has no liveness check, and `yoke health` can block on a FIFO | fixed in 6a98f22, 9df0602 (whitespace-preserving JSON changes, OS-held live locks, nonblocking descriptor checks and atomic exchange preserve and report a displaced concurrent save) | `crates/yoke-cli/src/install/` |
| G-7 | P3 | Plans | `yoke plan new` outside a repository, or in a bare one, says to make an initial commit, which cannot be followed there | fixed in 6a98f22 (missing repository, empty repository and bare repository guidance distinguished; CLI regression passed) | `crates/yoke-cli/src/workflow.rs` |
| G-11 | P3 | Guidance | every "trusted policy unavailable" denial ends with a storage hint, and a no-op Codex install repeats the hook review warning | fixed in 6a98f22, 9df0602 (policy denials have relevant guidance; actual Codex changes emit approval warning and no-op reinstall does not, with focused assertion) | `crates/yoke-cli/src/hook.rs`, `crates/yoke-cli/src/install.rs` |
| out-1 | P3 | CLI output | `yoke remember` and the `memory export` and `memory import` reports still print one JSON value, and `remember` includes epoch seconds | fixed in 6a98f22 (readable defaults, local timestamps and explicit `--json`; focused tests and isolated CLI execution passed) | `crates/yoke-cli/src/memory.rs` |
| out-2 | P3 | Plans | `yoke plan status` and `plan list` do not show the session a plan is bound to | fixed in 6a98f22 (text and JSON include bound sessions; list accepts `--json`; CLI regression passed) | `crates/yoke-cli/src/workflow.rs` |
| out-4 | P3 | Installer | the installer's migration step `yoke log --limit 1` now depends on the project of the working directory | fixed in a2b3428 (migration uses `log --all --limit 1`; installer regression passed) | `install.sh` |
| anchor-1 | P3 | Notifications | a stale trusted nested policy makes the notification sync log an error line, because its lookup uses the project root | fixed in 6a98f22, 9df0602 (nested trusted-policy notification lookup derives its own lookup scope and handles stale snapshots) | `crates/yoke-core/src/notifications.rs` |
| SEC-8 | P2 | Security | session-start memory injection was unlabeled, and an agent could plant and pin notes for any project | partly fixed in 6947257, 6a98f22, 9df0602 (untrusted fencing, human-only pinning, anchored agent memory scope, per-item cap and reserved injection budgets; trusted-only injection and memory authorship remain unimplemented) | `crates/yoke-core/src/memory.rs`, `crates/yoke-cli/src/memory.rs` |
| SEC-10 | P2 | Release | release authenticity was a same-origin SHA-256, and the source fallback built an unpinned branch | partly fixed in a42a20e (source builds use one exact revision, named before it is built, and releases gain `SHA256SUMS` and build provenance; the installer and the updater do not verify provenance, there is no detached signature, and the workflow has not run on a real release) | `install.sh`, `.github/workflows/release.yml` |
| SEC-14 | P2 | Security | gates and approved plan checks run project code outside the agent's sandbox, and approval freezes the command text, not the code | fixed in 6a98f22, 9df0602 (approval binds declared input hashes; gates inherit compatible resource limits with core dumps off, documented default environment and trusted pass_env, plus process-group wall timeout) | `docs/workflows.md`, `crates/yoke-core/src/engine.rs` |
| C13 | P2 | State | a zero-byte `state.db` silently became a fresh install, so trust was gone and enforcement was off | partly fixed in 6947257 (a loud warning on stderr and in `error.log`; failing closed was declined, see D2) | `crates/yoke-core/src/store.rs` |
| SEC-16 | P3 | Hooks | a very large tool input overflows the environment of a check gate | partly fixed in 6a98f22, 9df0602 (32 KiB marked gate input and truncation flag implemented; matchers see full input and NULs are scrubbed; troubleshooting table still describes the former environment-size denial) | `crates/yoke-core/src/engine.rs` |
| SEC-29 | P3 | Hardening | same-user races and stale state: FIFO opens, preset lock ownership, ambiguous memory markers, unlocked sharing and unbounded extension scans | fixed in 6a98f22 (locks, unambiguous entries and bounded extension reads verified; corrected FIFO/swap test passed with a checked child count, nonblocking hash descriptors and regular-file validation; earlier zero-test result withdrawn) | `crates/yoke-core/src/plan/files.rs`, `crates/yoke-core/src/memory.rs` |
| SEC-17 | P3 | Security | relative `PATH` entries and the Git, agent and shell commands resolve against the agent's directory | fixed in 6a98f22, 9df0602 (shared absolute executable lookup rejects relative/empty PATH entries, scrubs inherited Git overrides and disables fsmonitor/hooks/optional locks) | `crates/yoke-core/src/plan/files.rs`, `crates/yoke-core/src/review.rs` |
| SEC-19 | P3 | Updates | the daily release check has no opt-out, and the updater launched the `curl` found on `PATH` | fixed in 6a98f22, 9df0602 (prior transport fixes retained; trusted `update.check = false` suppresses scheduling and child launch, defaults on and merges explicitly; focused proof checked) | `crates/yoke-core/src/config.rs`, `crates/yoke-cli/src/hook.rs` |
| SEC-24 | P3 | Installer | the receipt and backups keep full copies of the agent settings, which can hold tokens, are never pruned and survive uninstall | fixed in 6a98f22, 9df0602 (receipts store owned entries/structure/hashes; bounded five private backups may still contain credentials and survive uninstall; dry-run hides unrelated values and pre-existing event names; expired backup ownership preserved) | `crates/yoke-cli/src/install/files.rs` |
| SEC-28 | P3 | Release | the release workflow used floating tools, `tags: ['*']` and no protected environment | partly fixed in a42a20e (tag filter, pinned toolchain and `cargo-deny`, deny in its own job, a green-CI gate, a protected `release` environment and reproducible archives; there is no scheduled advisory run or dependency-update automation, `ci.yml` still uses floating stable, and none of it has run) | `.github/workflows/release.yml` |
| S-3 | P3 | Scale | the first runs after an upgrade needed 124 notification syncs to read 500 thousand old decisions | fixed in 6a98f22, a2b3428 (bounded hooks queue one durable drain; windows skip deleted ids; focused queue/gap tests passed and an exact-count 20,000-row job drained in 261 ms; 700 MB performance remains unverified) | `crates/yoke-core/src/notifications.rs` |
| SEC-21 | P3 | Hooks | a hook killed with SIGKILL leaves a running gate command behind | declined (see the failure-mode decisions below, D5) | `crates/yoke-core/src/review/process.rs` |
| after-fix-2 | note | Usage | `yoke usage` showed only a weekly Codex window | declined: not a defect, Codex reported only a 7-day window on that plan | none |
| release-2 | P1 | Acceptance | install and update rehearsal instructions treated a temporary HOME as isolation for a shipped optimized executable, which can reach real state | fixed in 9df0602 (disposable account or container required; current acceptance summaries mark a safe rerun pending; account and container execution remain unrun) | `docs/release-acceptance.md` |
| j4-3 | P1 | Usage | a transcript over 32 MiB made a whole session's usage disappear, with only an anonymous error-log line | fixed in 6a98f22 (incremental accounting retained; per-session `usage_partial` notifications and the `yoke usage` recovery action verified; skipped older bytes still under-report as documented) | `crates/yoke-core/src/usage.rs`, `crates/yoke-core/src/notifications.rs`, `crates/yoke-cli/src/notifications.rs` |
| j3-memory-1 | P1 | Memory | `recall --explain` said session start injects notes, but observe mode injects nothing | fixed in ca1cb8f | `crates/yoke-cli/src/memory.rs` |
| j3-memory-2 | P1 | Memory | `forget`, `memory pin` and `share` took a global id and acted on other projects | fixed in ca1cb8f | `crates/yoke-core/src/memory.rs` |
| j3-memory-5 | P1 | Redaction | credentials inside URLs and multi-word values were stored in plain text | fixed in 250dd84 | `crates/yoke-core/src/redact.rs` |
| j4-1 | P1 | Statusline | the docs promised price estimates in the statusline, which never showed them; it now shows quota and the docs say so | fixed in ca1cb8f, 746cef9 | `crates/yoke-cli/src/statusline.rs`, `README.md` |
| j6-plans-console-1 | P1 | Console | `yoke console` exited when no browser could be opened and told the user to use the link | fixed in ca1cb8f | `crates/yoke-cli/src/console.rs` |
| j8-1 | P1 | Installer | the installer connected hooks, then failed on a newer state database, and every tool call was denied with a disk-space hint | fixed in ca1cb8f, 746cef9 | `install.sh`, `crates/yoke-cli/src/hook.rs`, `crates/yoke-cli/src/health.rs` |
| SEC-1 | P1 | Security | the project policy followed the agent's directory, so an untrusted nested `yoke.toml`, a directory outside the project or a symlink out of it turned the trusted policy off (same cause as G-1) | fixed in 6947257 (a session is pinned to the project it started in) | `crates/yoke-core/src/trust/anchor.rs`, `crates/yoke-cli/src/hook.rs` |
| SEC-2 | P1 | Security | an agent could switch Yoke off in a running session with `YOKE_OFF`, `HOME`, `disableAllHooks` or a Codex hook edit, and the guard protected none of those files | fixed in 6947257 (the guard covers hook registration files and the executable, `YOKE_OFF` works only with the private token from `yoke pause`, `yoke health` and the statusline flag the paths; installing into managed settings was not built, and no live session has confirmed the settings-reload behaviour the finding rests on) | `crates/yoke-core/src/guard.rs`, `crates/yoke-cli/src/bypass.rs` |
| SEC-3 | P1 | Security | `yoke uninstall`, `install` and `update` ran from an agent shell and could remove every hook | fixed in 6947257 (they refuse agent processes; this closes the casual path only) | `crates/yoke-cli/src/commands.rs`, `crates/yoke-core/src/guard.rs` |
| C2 | P1 | Hooks | with unreadable state a Stop was blocked on every try, so the agent looped until the state was fixed | fixed in 6947257 (blocked once; the repeated Stop is let through with a note) | `crates/yoke-cli/src/hook.rs` |
| C3 | P1 | Hooks | a closed or unread stderr made the hook panic with exit 101 and no answer, which failed open in enforce mode | fixed in 6947257 (exit 0 with the denial printed) | `crates/yoke-cli/src/hook.rs` |
| G-1 | P1 | Layouts | a nested untrusted `yoke.toml` replaced a trusted project's gates for every hook below it (same cause as SEC-1) | fixed in 6947257 (the nearest trusted project wins and the ignored file is listed) | `crates/yoke-core/src/trust.rs` |
| G-2 | P1 | Layouts | a linked Git worktree ran with no gates because its copy of `yoke.toml` starts untrusted | fixed in 6947257 (the policy trusted for the main repository applies until the worktree's own file is trusted; the trust-model extension needs the owner's confirmation) | `crates/yoke-core/src/trust.rs` |
| G-4 | P1 | Layouts | a payload directory that no longer existed denied every tool call, even in observe mode | fixed in 6947257 (the hook uses the directory it runs in) | `crates/yoke-cli/src/hook.rs` |
| release-1 | P2 | CI | the installer workflow did not build its tested revision, and its interactive statusline assertion could not fail | fixed in 85cad17 (passes `YOKE_COMMIT` and checks the installed build; all 11 jobs passed in [Installer run 39052847800](https://github.com/mrx-arafat/yoke/actions/runs/39052847800) at d5d4ed5) | `.github/workflows/installer.yml` |
| j1-2 | P2 | CLI output | `yoke log` lists every project by default while other status commands are project-scoped; agent, tool and project columns were added | fixed in 6947257 (`yoke log` lists the current project, `--all` lists every project, and the table has a session column) | `crates/yoke-cli/src/log_cmd.rs` |
| j1-7 | P2 | Notifications | the notification `doc` field is a repo-relative path that does not exist on an installed machine | fixed in 6947257 (full URLs in the notification `doc` field; the next step for a configuration waiting for trust is `yoke trust`) | `crates/yoke-core/src/notifications.rs`, `crates/yoke-cli/src/notifications.rs` |
| j4-5 | P2 | Notifications | same cause as j1-7, seen again in the usage journey | fixed in 6947257 (same cause as j1-7) | `crates/yoke-core/src/notifications.rs`, `crates/yoke-cli/src/notifications.rs` |
| after-fix-1 | P2 | Installer | the one-line installer did not set Claude Code's `statusLine`, so a user saw no Yoke line and `yoke usage` had no Claude quota | fixed in 8ce9b2d | `install.sh`, `crates/yoke-cli/src/install.rs` |
| j1-4 | P2 | Guidance | `gates` and `config` were silent about a `yoke.toml` waiting for trust | fixed in ca1cb8f, 6947257 (an empty `deny` or `would_deny` listing now says that no gate has run, and names an untrusted `yoke.toml`) | `crates/yoke-cli/src/commands.rs`, `crates/yoke-cli/src/log_cmd.rs` |
| j3-memory-6 | P2 | Memory | digest and enforce-mode injection were unreachable and the error did not say how to enable them | fixed in ca1cb8f, 6947257 (`yoke health` shows the number of pending undigested facts) | `crates/yoke-cli/src/memory.rs`, `crates/yoke-cli/src/health.rs` |
| j6-plans-console-5 | P2 | Plans | the documented way to find a session id for `plan start` is global and noisy | fixed in 746cef9, 6947257 (the default `yoke log` table has a session column; a default for `plan start --session` stays declined) | `crates/yoke-cli/src/log_cmd.rs`, `docs/workflows.md` |
| j1-1 | P2 | CLI output | `yoke ... \| head` panicked on a closed pipe and exited 101 | fixed in ca1cb8f | `crates/yoke-cli/src/main.rs` |
| j1-5 | P2 | Guidance | health guidance ignored the installer's global-policy route | fixed in ca1cb8f | `crates/yoke-cli/src/health.rs` |
| j1-6 | P2 | Guidance | bare `yoke` gave no health signal | fixed in ca1cb8f | `crates/yoke-cli/src/commands.rs` |
| j2-1 | P2 | CLI output | `yoke health` printed raw epoch seconds for `last_event` | fixed in ca1cb8f | `crates/yoke-cli/src/health.rs` |
| j2-2 | P2 | CLI output | the default `yoke log` had no agent, tool or command columns and used epoch times | fixed in ca1cb8f | `crates/yoke-cli/src/log_cmd.rs` |
| j2-3 | P2 | Updates | a daily "release check failed (exit status: 22)" in `error.log` while no release was published | fixed in ca1cb8f | `crates/yoke-cli/src/update.rs`, `crates/yoke-cli/src/update/transport.rs` |
| j2-4 | P2 | Installer | a global install records every session of the user's agents and the installer never said so | fixed in 746cef9 | `install.sh`, `docs/getting-started.md` |
| j2-5 | P2 | Statusline | the statusline showed no quota although Claude Code passes it | fixed in ca1cb8f | `crates/yoke-core/src/quota.rs`, `crates/yoke-cli/src/statusline.rs` |
| j3-memory-3 | P2 | Memory | `forget` left the shared copy in `.yoke/knowledge.md` and said nothing | fixed in ca1cb8f | `crates/yoke-core/src/memory.rs` |
| j3-memory-4 | P2 | Redaction | redaction masked ordinary words after any word containing token, secret or password | fixed in 250dd84 | `crates/yoke-core/src/redact.rs` |
| j3-memory-7 | P2 | Memory | restoring an export on the same machine needed an unexplained `--map P=P` | fixed in ca1cb8f | `crates/yoke-core/src/memory/portable.rs` |
| j3-memory-8 | P2 | Memory | an import conflict blamed a text difference when only the project differed, and `--dry-run` exited 0 | fixed in ca1cb8f | `crates/yoke-core/src/memory/portable.rs` |
| j4-2 | P2 | Usage | no command printed usage, tokens or cost | fixed in ca1cb8f | `crates/yoke-cli/src/usage_cmd.rs` |
| j4-4 | P2 | Updates | the daily release check blamed connectivity for a 404 and counted as a trial incident | fixed in ca1cb8f | `crates/yoke-cli/src/update/transport.rs` |
| j6-plans-console-2 | P2 | Console | no remote-server recipe and no way to choose the port | fixed in ca1cb8f, 746cef9 | `crates/yoke-cli/src/console.rs`, `docs/workflows.md` |
| j6-plans-console-3 | P2 | Console | a wrong `Host` showed "This launch link no longer works" and the advice was wrong | fixed in ca1cb8f | `crates/yoke-cli/src/console/launch.rs` |
| j6-plans-console-4 | P2 | Plans | state errors named the rule but not the plan's state or the next step | fixed in ca1cb8f | `crates/yoke-core/src/plan.rs` |
| j6-plans-console-6 | P2 | Plans | plans never hold Stop in observe mode and the flow never said so | fixed in ca1cb8f, 746cef9 | `crates/yoke-cli/src/workflow.rs`, `docs/workflows.md` |
| j6-plans-console-7 | P2 | Plans | `plan new` accepted a repository with no commit, then `plan diff` failed with raw Git text | fixed in ca1cb8f | `crates/yoke-core/src/plan.rs` |
| j7-extensions-codex-1 | P2 | Redaction | any name containing `sk-` (`ask-codex`, `task-runner`) was masked as an API key | fixed in 250dd84 | `crates/yoke-core/src/redact.rs` |
| j7-extensions-codex-2 | P2 | Extensions | the stale-hook repair advice named a step that `yoke install` refuses | fixed in ca1cb8f, 746cef9 | `crates/yoke-cli/src/install/drift.rs` |
| j7-extensions-codex-3 | P2 | Codex | Codex skips unapproved hooks silently and Yoke never connected that to "no events" | fixed in ca1cb8f | `crates/yoke-cli/src/health.rs`, `crates/yoke-cli/src/install/status.rs` |
| j7-extensions-codex-4 | P2 | Codex | Codex's sandbox failed on Ubuntu 24.04 and nothing in Yoke's docs said it is not Yoke | fixed in 746cef9 | `docs/troubleshooting.md` |
| j7-extensions-codex-5 | P2 | CLI output | `yoke log` hid agent, tool, project and session | fixed in ca1cb8f | `crates/yoke-cli/src/log_cmd.rs` |
| j8-2 | P2 | Installer | `executable changed; uninstall before reinstalling` named no command and no paths | fixed in ca1cb8f | `crates/yoke-cli/src/install.rs` |
| j8-3 | P2 | Updates | `yoke update --check` said "check connectivity" when no release was published | fixed in ca1cb8f | `crates/yoke-cli/src/update/transport.rs` |
| SEC-4 | P2 | Security | gate commands ran in the agent's directory, so a relative command resolved to a file the agent controls | fixed in 6947257 (gates run in the directory of the trusted `yoke.toml`) | `crates/yoke-core/src/engine.rs` |
| SEC-5 | P2 | Security | `~/.yoke`, `state.db` and `error.log` were created readable by other local users | fixed in 6947257 (directories 0700, files 0600, existing files tightened on open; `yoke health` does not report wider modes yet) | `crates/yoke-core/src/private.rs`, `crates/yoke-core/src/store.rs` |
| SEC-6 | P2 | Security | redaction missed many common credential shapes, and the log stored the output of tools | fixed in 6947257 (35 of the 39 audit shapes are masked and the log keeps the size and status of a tool's output; a secret with no keyword or prefix is still stored as typed, and old rows keep their text) | `crates/yoke-core/src/redact.rs` |
| SEC-9 | P2 | Security | a memory archive set pinned flags, timestamps and pending facts, and showed no text before import | fixed in 6947257 (import unpins unless `--keep-pins`, rejects out-of-range dates, shows a text preview with `--dry-run` and refuses an agent process) | `crates/yoke-core/src/memory/portable.rs` |
| SEC-11 | P2 | Security | the digest worker read a transcript path chosen by the hook payload, outside the agent's sandbox | fixed in 6947257 (the worker reads only a `.jsonl` regular file inside the project or an agent's transcript directory; there is no check when the job is queued) | `crates/yoke-cli/src/jobs.rs` |
| C1 | P2 | Hooks | observe mode denied every blocking event when the payload directory was relative | fixed in 6947257 (observe allows and records the error; enforce denies without the disk-space hint) | `crates/yoke-cli/src/hook.rs` |
| C4 | P2 | Hooks | any other panic ended the hook with exit 101 and no answer | fixed in 6947257 (converted into an error that enforce mode denies) | `crates/yoke-cli/src/hook.rs` |
| C5 | P2 | Hooks | SIGTERM ended the hook but left the gate command running | fixed in 6947257 (the gate's process group is killed; SIGKILL cannot be caught, see SEC-21) | `crates/yoke-core/src/review/process.rs` |
| C6 | P2 | State | a FIFO or a directory as `state.db` hung the hook or gave an unhelpful error | fixed in 6947257 (refused at once with a message that says to move it aside) | `crates/yoke-core/src/store.rs` |
| C9 | P2 | Guidance | recovery hints said to check free disk space for a damaged database, an unset `HOME` or a held lock | fixed in 6947257 | `crates/yoke-cli/src/hook.rs` |
| C11 | P2 | Trust | a damaged trust record made hooks deny and made `yoke trust` fail on the same error, so there was no way back | fixed in 6947257 (`yoke trust` replaces the record) | `crates/yoke-core/src/trust.rs` |
| G-3 | P2 | Layouts | `yoke init` in a repository subdirectory created a second project | fixed in 6947257 (it writes the project root's file and says where) | `crates/yoke-cli/src/commands.rs` |
| S-1 | P2 | Scale | `yoke trial report` took 1.9 to 3.6 s on 500 thousand decisions, and `yoke health` and filtered `yoke log` queries scanned the whole table | fixed in 6947257 (migration 013 and tighter queries: health 3 ms, trial report 0.4 s, log filters 4 ms) | `crates/yoke-core/src/store/013-scale-indexes.sql`, `crates/yoke-core/src/trial.rs` |
| S-2 | P2 | Scale | the hourly retention pass ran inside the SessionStart hook and cost 270 to 440 ms while a backlog existed | fixed in 6947257 (a background job deletes in short batches; the hook only queues it) | `crates/yoke-cli/src/retention.rs` |
| proc-1 | P2 | Tests | a test run with an unset `HOME`, and optimized builds run under an isolated `HOME`, wrote rows into a maintainer's real state database, once migrating it | fixed in 6947257, 0afac36, a2b3428 (CI now runs in a disposable home with offline update transport; an unset or relative `HOME` is an error in every build, only debug builds and `--features honor-home-env` honour `HOME`, and `CONTRIBUTING.md` says so; the leftover rows in that database are removed only with the maintainer's go-ahead) | `crates/yoke-cli/src/bypass.rs`, `CONTRIBUTING.md` |
| j1-14 | P3 | Guidance | `health: attention` is the normal state of a fresh install and is never explained | fixed in 6947257 (`yoke health` prints a `meaning` line under the status) | `crates/yoke-cli/src/health.rs` |
| j3-memory-14 | P3 | CLI output | `yoke log` is not project-scoped and shows other projects' tool payloads (same cause as j1-2) | fixed in 6947257 (same cause as j1-2) | `crates/yoke-cli/src/log_cmd.rs` |
| j8-5 | P3 | Storage | read-only commands fail with SQLite error 14 when `state.db` has no `-wal` and `-shm` files | fixed in 6947257 (a typed error names the missing files and `yoke log --limit 1`, and the read-only open no longer leaves a zero-byte `-wal`) | `crates/yoke-core/src/store.rs` |
| after-fix-3 | P3 | Usage | Codex reports `plan_type` and a credits balance; `plan_type` is only in `yoke usage --json` and the balance is not read | fixed in 6947257 (the plan shows in the quota table; the Codex credits balance is declined because it is a billing figure, not a rate-limit window) | `crates/yoke-core/src/quota.rs`, `crates/yoke-cli/src/usage_cmd.rs` |
| after-fix-4 | P3 | Usage | the compact `quota[n]{...}` table reads better for scripts than for people | fixed in 6947257 (the quota block is an aligned table; the structured records stay in `--json`) | `crates/yoke-cli/src/usage_cmd.rs` |
| j4-9 | P3 | Statusline | nothing pointed a new user at the statusline, and it cannot show notifications | fixed in ca1cb8f, 746cef9, 6947257 (the session-start hook syncs notifications and the statusline shows `! N unread`; the installer step is after-fix-1) | `crates/yoke-cli/src/health.rs`, `crates/yoke-cli/src/statusline.rs` |
| SEC-26 | P3 | Usage | one crafted statusline input could pin a forged quota window indefinitely | fixed in 6a98f22 (reset bounded by converted observation time plus window and five-minute slack; parsing and persistence regressions pass) | `crates/yoke-core/src/quota.rs` |
| S-4 | P3 | Scale | the default usage view scanned all usage samples | fixed in 6a98f22 (exact session/model/day summaries with indexed boundary reads; independently measured on a database under 50 MB; raw samples retained) | `crates/yoke-core/src/usage.rs`, `crates/yoke-core/src/store/015-usage-summaries.sql` |
| S-5 | P3 | Scale | the query planner had no maintained statistics | fixed in 6a98f22 (bounded `PRAGMA optimize` with analysis limit 400 during upgrades only; query plans measured) | `crates/yoke-core/src/store.rs` |
| health-1 | P3 | Notifications | notification links followed `main` even for released builds | fixed in 6a98f22 (matching tag-build metadata pins the version; local/manual builds keep the main fallback; tagged artifact execution remains unverified) | `crates/yoke-core/src/notifications.rs` |
| j1-3 | P3 | CLI output | timestamps were raw Unix seconds in the human tables | fixed in ca1cb8f, 6947257 (`yoke recall` shows local time and the plan commands print text; `remember` and archive reports gained text defaults in 6a98f22, see out-1) | `crates/yoke-core/src/timefmt.rs` |
| j1-8 | P3 | CLI output | `yoke log` filters matched exactly and failed silently | fixed in ca1cb8f | `crates/yoke-cli/src/log_cmd.rs` |
| j1-9 | P3 | Guidance | health and config listed policy files that do not exist as things to trust | fixed in ca1cb8f | `crates/yoke-core/src/trust.rs` |
| j1-10 | P3 | CLI output | errors printed twice and the help line was a placeholder | fixed in ca1cb8f | `crates/yoke-cli/src/main.rs` |
| j1-11 | P3 | Installer | `yoke install --dry-run` printed one escaped JSON string per file | fixed in ca1cb8f | `crates/yoke-cli/src/preview.rs` |
| j1-12 | P3 | Plans | `yoke plan new` printed raw JSON | fixed in ca1cb8f | `crates/yoke-cli/src/workflow.rs` |
| j1-13 | P3 | Guidance | the starter file shipped a placeholder project name | fixed in ca1cb8f | `crates/yoke-cli/src/starter.rs` |
| j2-6 | P3 | Installer | `ssh host yoke ...` failed after install because the PATH line is skipped by non-interactive shells | fixed in 746cef9 (the installer and docs say so; the PATH line stays in the interactive startup file) | `install.sh`, `docs/getting-started.md` |
| j2-7 | P3 | Codex | Codex's missing-bubblewrap warning on Linux servers was undocumented | fixed in 746cef9 | `docs/troubleshooting.md` |
| j3-memory-9 | P3 | Memory | `recall` never said which project it searched | fixed in ca1cb8f | `crates/yoke-cli/src/memory.rs` |
| j3-memory-10 | P3 | CLI output | every error printed its message twice plus a generic help line | fixed in ca1cb8f | `crates/yoke-cli/src/main.rs` |
| j3-memory-11 | P3 | Memory | missing or repeated ids behaved inconsistently across `forget`, `share` and `pin` | fixed in ca1cb8f (a notice for a duplicate `remember` was declined) | `crates/yoke-cli/src/memory.rs` |
| j3-memory-12 | P3 | Memory | redaction broke the stored fact JSON for a file write | fixed in ca1cb8f | `crates/yoke-core/src/memory.rs` |
| j3-memory-13 | P3 | Memory | `--explain` footer was wrong for a query and scores printed `-0.000` | fixed in ca1cb8f (raw epoch in `.yoke/knowledge.md` was declined) | `crates/yoke-cli/src/memory.rs` |
| j4-6 | P3 | CLI output | raw Unix epochs in `log`, `notifications`, `health` and `jobs` | fixed in ca1cb8f, 6947257 (same remainder as j1-3) | `crates/yoke-core/src/timefmt.rs` |
| j4-7 | P3 | Statusline | invalid statusline input printed an error block into the prompt bar | fixed in ca1cb8f | `crates/yoke-cli/src/statusline.rs` |
| j4-8 | P3 | Usage | rate limits were in both agents' data and Yoke showed neither | fixed in ca1cb8f | `crates/yoke-core/src/quota.rs` |
| j4-10 | P3 | Installer | `yoke install --dry-run` printed whole files as one escaped string (same cause as j1-11) | fixed in ca1cb8f | `crates/yoke-cli/src/preview.rs` |
| j6-plans-console-8 | P3 | CLI output | an error printed twice on a terminal | fixed in ca1cb8f | `crates/yoke-cli/src/main.rs` |
| j6-plans-console-9 | P3 | Plans | output mixed compact tables with one-line JSON and empty diff entries | fixed in ca1cb8f | `crates/yoke-cli/src/workflow.rs`, `crates/yoke-cli/src/diff.rs` |
| j6-plans-console-10 | P3 | Plans | `plan status` hid replies and showed a standalone review as a passing check on a draft | fixed in ca1cb8f | `crates/yoke-cli/src/workflow.rs` |
| j6-plans-console-11 | P3 | Plans | `yoke health` gave no guidance for a draft plan | fixed in ca1cb8f | `crates/yoke-cli/src/health.rs` |
| j7-extensions-codex-6 | P3 | Log | `yoke log --json` payloads were truncated and not valid JSON | fixed in ca1cb8f (a nested payload object was declined; `payload_truncated` marks cut rows) | `crates/yoke-cli/src/log_cmd.rs` |
| j7-extensions-codex-7 | P3 | Extensions | the drift table was buried under identical symlink notes | fixed in ca1cb8f | `crates/yoke-cli/src/extensions.rs` |
| j7-extensions-codex-8 | P3 | Extensions | the text view did not say what a hook or MCP row runs | fixed in ca1cb8f | `crates/yoke-cli/src/extensions.rs` |
| j7-extensions-codex-9 | P3 | Extensions | any shadowed definition made the exit code 1 (policy change, see open items) | fixed in ca1cb8f, 746cef9 | `crates/yoke-core/src/extensions.rs` |
| j7-extensions-codex-10 | P3 | Guidance | "untrusted (ignored)" was printed for files that do not exist | fixed in ca1cb8f | `crates/yoke-cli/src/hook.rs` |
| j7-extensions-codex-11 | P3 | CLI output | one error printed three lines and the last one was wrong | fixed in ca1cb8f | `crates/yoke-cli/src/main.rs` |
| j7-extensions-codex-12 | P3 | Redaction | ordinary words were masked next to `Secrets` or `password` in logged tool output | fixed in 250dd84 | `crates/yoke-core/src/redact.rs` |
| j8-4 | P3 | Uninstall | `command -v yoke` still printed the removed path because of the shell's command cache | fixed in 746cef9 | `docs/uninstalling.md` |
| j8-6 | P3 | Installer | a reinstall from another directory left the old executable, which could win on PATH | fixed in 746cef9 | `install.sh` |
| j8-7 | P3 | Installer | the PATH line was unreachable for non-interactive shells and the wording hid it | fixed in 746cef9 | `install.sh` |
| j8-8 | P3 | Uninstall | uninstall left a blank `~/.zshrc`, and the decision-log export was world-readable | fixed in 746cef9 | `docs/uninstalling.md` |
| j8-9 | P3 | Installer | health explained the broken state but not the fix, and receipt errors named no path | fixed in ca1cb8f | `crates/yoke-cli/src/install/status.rs`, `crates/yoke-cli/src/install.rs` |
| j8-10 | P3 | Installer | the non-interactive failure suggested the broad `YOKE_YES=1` instead of `YOKE_INSTALL_RUST=1` | fixed in 746cef9 | `install.sh` |
| SEC-15 | P3 | Security | `error.log` had no size cap and was readable by other users | fixed in 6947257 (mode 0600; moved to `error.log.1` past 1 MiB; same fix as C15) | `crates/yoke-cli/src/hook.rs` |
| SEC-25 | P3 | Installer | `install.sh` accepted plain-http mirrors and downloaded without protocol, time or size limits | fixed in a42a20e (https only except loopback, curl and wget limits, unsafe install directories refused; the rustup pipe and the script header fetched from `main` are documented, not changed) | `install.sh` |
| C7 | P3 | Gates | a very large tool input or a NUL in a session id overflowed a gate's environment and denied with an unexplained errno | fixed in 6947257 (the values are cleaned and the reason is stated; see SEC-16) | `crates/yoke-core/src/engine.rs` |
| C8 | P3 | Hooks | an oversized payload cut off the writer and the message offered only the bypass; a byte-order mark before the JSON was a parse error | fixed in 6947257 (the hook reads and drops the rest, the message says to use smaller tool inputs, and the mark is accepted) | `crates/yoke-cli/src/hook.rs` |
| C10 | P3 | Guidance | in observe mode the denial reason talked about restarting without Yoke although nothing was blocked | fixed in 6947257 | `crates/yoke-cli/src/hook.rs` |
| C12 | P3 | Trust | a symlinked `yoke.toml` gave an operating-system error at trust time | fixed in 6947257 | `crates/yoke-core/src/trust.rs` |
| C14 | P3 | State | a claim stamp left by a clock set back delayed retention and the update check | fixed in 6947257 | `crates/yoke-core/src/store.rs` |
| C15 | P3 | Hooks | `error.log` grew without bound | fixed in 6947257 (same fix as SEC-15) | `crates/yoke-cli/src/hook.rs` |
| C16 | P3 | Gates | gate output throughput was capped by a 5 ms sleep | fixed in 6947257 | `crates/yoke-core/src/review/process.rs` |
| C17 | P3 | Gates | a spawn error did not say whether the shell or the working directory was missing | fixed in 6947257 | `crates/yoke-core/src/engine.rs` |
| G-5 | P3 | Layouts | a Git repository at the home directory made everything below it one project | fixed in 6947257 | `crates/yoke-core/src/trust.rs` |
| G-6 | P3 | CLI output | `yoke config` did not say which project it resolved | fixed in 6947257 (it prints `project:`) | `crates/yoke-cli/src/commands.rs` |
| j2-8 | note | Installer | until a policy is trusted no gate runs, and the installer's closing lines did not make that the next step | fixed in 746cef9 | `install.sh` |

Declined parts of otherwise fixed findings:

- j3-memory-11: `yoke remember` gives no notice for identical text.
  Identical notes are harmless and deduplication needs a policy decision.
- j3-memory-13: the epoch in `.yoke/knowledge.md` stays.
  The hardened entry format indents note content and recognizes complete legacy entries so existing shared notes are not orphaned.
- j3-memory-2: ids are still table row ids and can be reused after a delete.
  Project scoping removes the cross-project hazard, and a stable id needs a schema migration.
- j7-extensions-codex-6: `payload` stays a string in `--json`; the new `payload_truncated` field marks the cut rows.
- j3-memory-10: the structured `error:` and `help:` lines stay on standard output in every mode, because agents read them there.
- after-fix-3: the Codex credits balance is not read.
  It is a billing figure rather than a rate-limit window, and reading it would put account money into the state database and into any captured output.
- SEC-6: a secret with no keyword or prefix, such as a bare hex string, a base64 blob or an AWS secret key without its variable name, is stored as typed.
  No pattern can tell it from a commit hash or a checksum.
- SEC-9: a typed confirmation for a memory import was not added; the refusal for agent processes and the `--dry-run` text preview carry that role.
- SEC-10: the installer and `yoke update` do not verify build provenance themselves.
  It needs a verifier or `gh` plus authentication, it cannot be tested before a first release, and [verifying releases](verifying-releases.md) documents the manual check.
- G-5: a Git repository at the home directory is not a project, so a person whose only repository is there gets the subdirectory as the project unless a `yoke.toml` sits in the home directory.

## Open items

Everything below is open, partly fixed, declined or waiting for a person.
Each item names its row in the [findings ledger](#findings-ledger); this section groups them and adds what a row cannot hold.
No P1 is open.

### Release blockers, in priority order

1. Commit and verify the digest input-validation ordering fix in hosted CI; Installer passed all 11 jobs at `9df0602`, while both hosted test jobs failed the FIFO case.
2. After CI is green, investigate Codex samples missing timestamps and recover per-day attribution where possible (S-6); reconcile the SEC-16 troubleshooting text, SEC-23 shell precheck proposal and remaining policy items below.
3. Coordinate the schema-15 installed-binary upgrade, then test current-source enforcement and a real hook payload cwd transition with both agents.
4. Complete shipped-artifact rehearsal in disposable accounts or containers, human-only release gates, observe-day and enforce-week trials, repository settings and console acceptance.
5. Establish optimized hook latency against the large-database target; current bounded debug-process medians exceed the p50 target and the 700 MB timings were not rerun.

The implementation packages and their follow-ons are committed; that does not establish release acceptance.
S-6 is the one fully open row, and the partial items below retain their specific limits.

### P2, open

- S-6: determine why the Codex importer leaves sample timestamps empty, recover event times from rollout records where possible, and verify per-day totals after backfill.
  The schema-15 rehearsal preserved session totals but exposed this gap in daily attribution.
  This follow-up waits until CI is green.

### Partly fixed, with what remains

- SEC-8: agent memory scope and injection caps/reserved budgets are implemented; trusted-only injection and memory authorship remain unimplemented.
  Pins that an agent made before the fix cannot be told apart, so the [upgrade notes](upgrade-notes.md) ask people to review their pins.
- SEC-10: decide whether the installer and updater verify build provenance or leave it a documented manual step, add a detached signature, and run the release workflow once to prove the attestation, the `release` environment and the gate.
  Reviewers, tag protection and branch protection are repository settings that the code cannot show.
- SEC-16: bounded marked input and `YOKE_TOOL_INPUT_TRUNCATED` are implemented and described in [configuration](configuration.md).
  The troubleshooting failure table still describes the former environment-size denial and needs reconciliation.
- SEC-23: executable registration and health checks are implemented, including sticky-directory protection and group-write guidance.
  The shell installer's explicit regular-executable check before execution remains an unapplied proposal.
- SEC-28: scheduled advisory checks, dependency-update automation and a pinned CI toolchain remain undone.
  The first two are a maintainer's policy choice; release workflow execution and repository protection also need acceptance.
- C13: a zero-byte `state.db` still falls back to defaults with a loud warning (see D2 below).

### Not verified live

These rest on tests, isolated debug builds, reading code or documentation, not on a real agent session.

- The session anchor after an actual change to hook payload cwd, the human `yoke pause` token flow, and whether a settings file's `env` block reaches hooks and is reloaded in a running session (SEC-2).
  The current Linux field test confirmed protected-settings denials and that `YOKE_OFF=1` without a token does not disable enforcement, but did not exercise those remaining cases.
- The statusline styles in a real Claude Code session: colors, the terminal width, and a second line.
- The escaped plan approval preview and shared-note console disclosure in human interaction.
  Focused source/action tests cover the changes; no browser or human approval was run in this wave.
- `yoke watch` in a Linux terminal emulator.
  A non-terminal Linux snapshot passed; the interactive view was driven through a pseudo-terminal on macOS only.
- Linux behaviour of the failure-mode pass: the environment size limit for a gate, a full ext4 disk, and the file modes of private state.
- `yoke pause` end to end, and the human-only plan commands `approve`, `resume`, `cancel` and `reassign`: they refuse agent processes, so only their parts and renderers have tests.
- The release-profile behaviour of `honor-home-env`: it is tested through its decision function, not by running an optimized binary.
- Whether the review and digest child agents keep Yoke's hooks out, now that hooks refuse `YOKE_OFF=1`: Claude children start with no setting sources, and the Codex flag was read from code, not tested.
- The release workflow: `actions/attest`, the `release` environment, the green-CI gate, the `SHA256SUMS` step and `gh attestation verify` have not run, because they need a tag or a dispatch.
- The wget path of `install.sh`, and the installer's sudo prompt for a missing C compiler on a real server with a password-protected `sudo`.
  CI covers `sudo` without a password and a terminal driver as root, and the field-test server already had a compiler.
- Console items on a remote server: a browser through an SSH tunnel, the wrong-address page, and the console acceptance pass.
- Codex hook approval after `yoke uninstall` and `yoke install`: a rehearsal in an isolated home shows that the approval key survives where Yoke's group is last in an event's list and changes where another tool's group follows it.
  The behaviour of Codex itself on a changed key comes from its documentation, not from a run; [troubleshooting](troubleshooting.md) describes it.

### Free-model work

- The audit of panics in production code finished, but a free model graded its own hits.
  None of its labels was independently checked, its summary counts (112 SAFE, 8 RISKY, 3 FIX-NOW) do not match its own table, and at least one FIX-NOW label calls its own invariant safe.
- The check that every command in the documentation runs as written (a free model) and the audit of user-facing error messages (a free model) produced no report.
  The error audit left only a raw evidence file.

### Declined after the failure-mode tests

These were considered and left as they are, so that nobody reopens them without new evidence.

- w2-chaos D2: a zero-byte or wiped `state.db` makes enforcement silently fall back to observe, because the trust rows are gone.
  It now warns loudly on stderr and in `error.log`; failing closed while an old empty file exists would deny after a harmless crash at create, and deleting the file by hand cannot be told from a wipe.
- w2-chaos D3: a payload over 1 MiB is denied in enforce mode.
  The message tells the agent to use smaller tool inputs.
- w2-chaos D4: alternate stdin delivery was declined.
  Since `6a98f22`, check gates instead receive at most 32 KiB with an explicit truncation marker and flag; matchers, `deny` and `rewrite` gates still see the full input.
- w2-chaos D5: a hook killed with SIGKILL still leaves a running gate command behind; a pid ledger that the next hook reaps was declined as more machinery than the case is worth, and gate timeouts are the bound.
- w2-chaos D6: a Stop denied only because Yoke failed is let through the second time (`stop_hook_active`); a stricter counter that survives without the database was declined as heavier than the risk.
- w2-chaos D7: a read-only `HOME` with no `~/.yoke` still denies blocking events, because the state and the last known mode cannot be read there.
- Also declined in the failure-mode pass: a shorter SQLite busy timeout for hooks (50 concurrent hooks would lose audit rows), rejecting a NUL in a gate command at trust time, and handling payloads nested deeper than 128 levels.
- The installer adds the PATH line only after a prompt or with `YOKE_YES=1`.
  A piped install cannot ask, the startup file belongs to the user, and `yoke uninstall` does not remove the line, so an unprompted edit would leave something Yoke cannot clean up.

### Decisions and checks that need a person

- Confirm the exit-code policy from j7-extensions-codex-9.
  Decision 0003 was amended so that only `stale` and `dangling` findings exit 1 and a `shadowed` name exits 0.
  The revert is one line, `Drift::fails_check`.
- Confirm that a linked worktree applies the policy trusted for its repository until its own file is trusted (G-2).
  This extends the trust model; the revert is to make `trusted_row` return the worktree's own snapshot only.
- Choose the memory identity of a Git worktree.
  Today each worktree has its own memory, plans and log while the policy is shared.
  Option A makes the main checkout's path the identity, option B keeps the current behaviour and documents it, option C adds a project id to `yoke.toml`.
  The layout pass recommends B now and A as a planned change.
- G-10 now treats a non-UTF-8 project scope as unavailable and keeps trusted parent/global policy.
  Native Linux filename acceptance remains unverified.
- Decide whether session-start notes are injected by default in enforce mode, or only approved knowledge (`memory.inject = "trusted-only"`), and whether PostToolUse output may be stored at all (it now stores only size and status).
- Decide whether to offer an install into Claude Code's managed settings, which survive `disableAllHooks`.
- Decide whether the statusline should rank the quota above the plan when the terminal is narrow.
  A full line is about 215 columns, so at 120 to 160 columns the quota, cost and context segments are often the ones dropped.
- Decide on dependency-update automation for the repository (SEC-28).
- Verify the likely remedy for the Codex sandbox on Ubuntu 24.04, installing the distribution's `bubblewrap` package.
  The troubleshooting section marks it unverified.
- Have a person run the plan commands that refuse agent processes (`approve`, `resume`, `cancel`, `reassign`) once in a scratch project and read their output.
- Approve the removal of the test rows that earlier runs left in a maintainer's real state database (proc-1).
  Removing them is a destructive database operation; the statements were rehearsed on a copy and wait for the maintainer's go-ahead.

## How to run the next field test

1. Prepare a fresh server, ideally Ubuntu 24.04 x86_64 as before, with a non-root user, Claude Code and Codex installed and logged in, and no Yoke files.
   Record the commit or tag under test and the SHA-256 of the installed executable, because `yoke --version` only prints `0.1.0`.
2. Install with the one-line installer from the [README](../README.md#quickstart), at a terminal, and keep the output verbatim.
   Then rerun it to confirm that nothing changes.
3. Walk the seven journeys, one agent or person per journey, each in its own directory.
   Each journey writes a report with the commands and real output, then findings, then the steps only a human may run.
   Start with the steps no test has run yet, listed under [Not verified live](#not-verified-live), and with the second-wave changes that no real agent session has exercised.
   - First run: a fresh login, `yoke`, `yoke --help`, `yoke health`, `yoke init` in a Git project, `yoke gates`, `yoke config`, `yoke log`, `yoke notifications`, one real `claude -p` and one `codex exec` run, and a few mistyped commands.
   - Live agent sessions: interactive Claude Code and Codex in a demo project, the Codex `/hooks` approval, the statusline, and `yoke log` afterwards.
   - Memory: `remember`, `recall`, `memory pin`, `share`, `forget`, export and import, inputs that exercise redaction (URL passwords, `key: value` lines and prose near the word token), and a real session with and without enforce mode.
   - Usage and notifications: run both agents, compare `yoke usage` with each agent's own totals, feed a real statusline callback to `yoke statusline`, and read `yoke notifications` and `~/.yoke/error.log`.
   - Plans and console: `yoke plan new`, `validate`, `review` and the refused transitions; then, with a person at the keyboard, `yoke trust`, `yoke plan approve`, a real Stop hold in enforce mode, and `yoke console --no-open` through an SSH tunnel.
   - Extensions and Codex: `yoke extensions` with shadowed, stale and dangling fixtures, the stale-hook repair, and `codex exec` with approved and unapproved hooks.
   - Install lifecycle: rerun, move the executable, an old and a newer state database, [uninstall](uninstalling.md) in order, and reinstall.
4. Keep the machine's real state safe.
   Run anything that installs, uninstalls or migrates in a throwaway home: `T=$(mktemp -d) && export HOME="${T:?}"`, then check `test -n "$HOME" && test -d "$HOME"` and that it is not your real home before every call, and unset `CLAUDE_CONFIG_DIR` and `CODEX_HOME`.
   Read the real database only with `sqlite3 'file:<db>?mode=ro'` or a `.backup` copy.
   Never run `yoke trust` or `yoke console` from an agent; a person runs them in their own terminal.
5. Record the result in this file as a new dated section, `Field test YYYY-MM-DD`, placed above the previous one.
   Give it the environment, the commit under test, the journeys walked, what was verified and what was not, and new findings with the same columns as the ledger.
   Number findings per journey, such as `j3-memory-15`, and update the status of earlier findings in place instead of repeating them.
   Then refresh the open items, the counts and the [development handoff](next-steps.md).
