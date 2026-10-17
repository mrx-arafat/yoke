# Field testing

[Documentation index](README.md) · [Release acceptance](release-acceptance.md) · [Development handoff](next-steps.md)

A field test walks Yoke the way a new user meets it: a fresh machine, the one-line installer, real Claude Code and Codex sessions, and every journey from first run to uninstall.
It differs from CI, which proves that code and the installer behave on clean images, and from [release acceptance](release-acceptance.md), which gates a published release on evidence that a person must supply.
A field test does not pass or fail.
It produces findings about the experience, such as wrong guidance, silent failures and misleading documentation, and this page keeps them.

This page records what was tested, how Yoke behaves in real use, every finding with its status, and how to repeat the test.
It is a record of observations, not a release acceptance claim.

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
After the fixes, 68 findings were fixed, 5 partly fixed and 6 open.

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

Not verified live:

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

- The first session starts after an upgrade from a build without notifications also read old history: each hook takes a window of 2,000 decision ids, the `yoke notifications` command 20,000.
  Draining 500 thousand decisions therefore takes about 250 session starts or 13 runs of the command.
  A window of 20,000 in the hook took 350 to 620 ms per hook (unoptimized build); 2,000 takes 40 to 120 ms there.
- Stop re-reads the whole transcript.
  It took 35 to 60 ms for a 5 MB transcript and grows with the file.
- `yoke usage` grows linearly with the number of samples, because usage is never pruned.
- Cold-cache first runs after a reboot are several times slower than the numbers above for any command that scans a table.

## Findings ledger

One row per finding, sorted by status and then severity.
Status is `fixed in <short sha>`, `partly fixed in <short sha>` with what remains in brackets, `open`, `fix in progress` or `declined`.
`open (not addressed in wave 1)` means no fix report mentions the finding.
The `after-fix` rows are observations from the first check of the fixed build, not journey findings.
Several findings share a cause, such as the log scope (j1-2 and j3-memory-14) and the notification link (j1-7 and j4-5).

| ID | Severity | Area | Summary | Status | Where |
| --- | --- | --- | --- | --- | --- |
| j1-2 | P2 | CLI output | `yoke log` lists every project by default while other status commands are project-scoped; agent, tool and project columns were added | open | `crates/yoke-cli/src/log_cmd.rs` |
| j1-7 | P2 | Notifications | the notification `doc` field is a repo-relative path that does not exist on an installed machine | open (not addressed in wave 1) | `crates/yoke-core/src/notifications.rs` |
| j4-5 | P2 | Notifications | same cause as j1-7, seen again in the usage journey | open (not addressed in wave 1) | `crates/yoke-core/src/notifications.rs` |
| j1-14 | P3 | Guidance | `health: attention` is the normal state of a fresh install and is never explained | open (not addressed in wave 1) | `crates/yoke-cli/src/health.rs` |
| j3-memory-14 | P3 | CLI output | `yoke log` is not project-scoped and shows other projects' tool payloads (same cause as j1-2) | open | `crates/yoke-cli/src/log_cmd.rs` |
| j8-5 | P3 | Storage | read-only commands fail with SQLite error 14 when `state.db` has no `-wal` and `-shm` files | open (not addressed in wave 1) | `crates/yoke-core/src/store.rs` |
| after-fix-3 | P3 | Usage | Codex reports `plan_type` and a credits balance; `plan_type` is only in `yoke usage --json` and the balance is not read | open | `crates/yoke-core/src/quota.rs`, `crates/yoke-cli/src/usage_cmd.rs` |
| after-fix-4 | P3 | Usage | the compact `quota[n]{...}` table reads better for scripts than for people | open | `crates/yoke-cli/src/usage_cmd.rs` |
| after-fix-1 | P2 | Installer | the one-line installer did not set Claude Code's `statusLine`, so a user saw no Yoke line and `yoke usage` had no Claude quota | fixed in 8ce9b2d | `install.sh`, `crates/yoke-cli/src/install.rs` |
| j4-3 | P1 | Usage | a transcript over 32 MiB made a whole session's usage disappear, with only an anonymous `error.log` line | partly fixed in ca1cb8f (newest 32 MiB counted and marked partial; incremental import and a notification remain) | `crates/yoke-core/src/usage.rs` |
| j1-4 | P2 | Guidance | `gates` and `config` were silent about a `yoke.toml` waiting for trust | partly fixed in ca1cb8f (help lines added; `log --outcome would_deny` with zero rows still says nothing about trust) | `crates/yoke-cli/src/commands.rs` |
| j3-memory-6 | P2 | Memory | digest and enforce-mode injection were unreachable and the error did not say how to enable them | partly fixed in ca1cb8f (message, docs and starter block; pending undigested facts are still invisible) | `crates/yoke-cli/src/memory.rs` |
| j6-plans-console-5 | P2 | Plans | the documented way to find a session id for `plan start` is global and noisy | partly fixed in 746cef9 (the workflow doc uses the project filter; the default `yoke log` table still has no session id) | `docs/workflows.md` |
| j4-9 | P3 | Statusline | nothing pointed a new user at the statusline, and it cannot show notifications | partly fixed in ca1cb8f, 746cef9 (health guidance and docs; the unread count was declined; the installer step is after-fix-1) | `crates/yoke-cli/src/health.rs` |
| after-fix-2 | note | Usage | `yoke usage` showed only a weekly Codex window | declined: not a defect, Codex reported only a 7-day window on that plan | none |
| j3-memory-1 | P1 | Memory | `recall --explain` said session start injects notes, but observe mode injects nothing | fixed in ca1cb8f | `crates/yoke-cli/src/memory.rs` |
| j3-memory-2 | P1 | Memory | `forget`, `memory pin` and `share` took a global id and acted on other projects | fixed in ca1cb8f | `crates/yoke-core/src/memory.rs` |
| j3-memory-5 | P1 | Redaction | credentials inside URLs and multi-word values were stored in plain text | fixed in 250dd84 | `crates/yoke-core/src/redact.rs` |
| j4-1 | P1 | Statusline | the docs promised price estimates in the statusline, which never showed them; it now shows quota and the docs say so | fixed in ca1cb8f, 746cef9 | `crates/yoke-cli/src/statusline.rs`, `README.md` |
| j6-plans-console-1 | P1 | Console | `yoke console` exited when no browser could be opened and told the user to use the link | fixed in ca1cb8f | `crates/yoke-cli/src/console.rs` |
| j8-1 | P1 | Installer | the installer connected hooks, then failed on a newer state database, and every tool call was denied with a disk-space hint | fixed in ca1cb8f, 746cef9 | `install.sh`, `crates/yoke-cli/src/hook.rs`, `crates/yoke-cli/src/health.rs` |
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
| j1-3 | P3 | CLI output | timestamps were raw Unix seconds in the human tables | fixed in ca1cb8f | `crates/yoke-core/src/timefmt.rs` |
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
| j4-6 | P3 | CLI output | raw Unix epochs in `log`, `notifications`, `health` and `jobs` | fixed in ca1cb8f | `crates/yoke-core/src/timefmt.rs` |
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
| j2-8 | note | Installer | until a policy is trusted no gate runs, and the installer's closing lines did not make that the next step | fixed in 746cef9 | `install.sh` |

Declined parts of otherwise fixed findings:

- j3-memory-11: `yoke remember` gives no notice for identical text.
  Identical notes are harmless and deduplication needs a policy decision.
- j3-memory-13: the epoch in `.yoke/knowledge.md` stays, because `forget` finds the entry by its exact text and a new format would orphan older entries.
- j3-memory-2: ids are still table row ids and can be reused after a delete.
  Project scoping removes the cross-project hazard, and a stable id needs a schema migration.
- j7-extensions-codex-6: `payload` stays a string in `--json`; the new `payload_truncated` field marks the cut rows.
- j3-memory-10: the structured `error:` and `help:` lines stay on standard output in every mode, because agents read them there.

## Open items

Everything below is open or in progress.
P1 has none left, apart from the remainder of j4-3.

### P2

  Open question: without a terminal and without `YOKE_YES=1` the installer only prints the command, so decide whether it should add the line when none exists, since the change is additive and `yoke uninstall` removes it.
- j1-2 and j3-memory-14: default `yoke log` to the current project, add `--all` for the global view, and update `docs/cli.md` and `docs/getting-started.md`.
- j1-7 and j4-5: emit full URLs for the notification `doc` field and show `yoke trust` as the human next step, then update the anchor test in `crates/yoke-core/src/notifications/tests.rs`.
- j4-3 (remainder): import transcripts incrementally from a stored byte offset, which also removes the 32 MiB re-read on every Stop, and raise a `usage_partial` notification per partly counted session.
- j1-4 (remainder): make `yoke log --outcome would_deny` with zero rows and no trusted gate say that no gate has run.
- j3-memory-6 (remainder): show the number of pending undigested facts in `yoke health` or `yoke recall`.
- j6-plans-console-5 (remainder): show the session id in the default `yoke log` table.
  A default for `plan start --session` was declined, because binding the wrong session would gate the wrong Stop.

### P3

- j1-14: define `attention` in the health output, for example "setup is incomplete, nothing is broken".
- j8-5: when `state.db` has no sidecars, say to run `yoke log --limit 1` once to recreate them, or open it writable.
  The failed read-only open also leaves a zero-byte `state.db-wal` behind.
- j4-9 (remainder): run the notification sync from the session-start hook, then add `! N unread` to the statusline.
  Today the count would read 0 in exactly the case it should flag.
- after-fix-3: show `plan_type` in the `yoke usage` table and decide whether the Codex credits balance belongs there.
- after-fix-4: render `yoke usage` quota as a table for people and keep the compact form behind `--json`.
- j1-3 and j4-6 (remainder): `yoke recall` still prints its time column as epoch seconds, and the plan commands other than `new`, `validate`, `comment`, `reply` and `review` still print one JSON value.
- j3-memory-3 (remainder): forgetting a shared note in the console leaves its entry in `.yoke/knowledge.md`; the console should say so.

### Declined after the failure-mode tests

These were considered and left as they are, so that nobody reopens them without new evidence.

- w2-chaos D4: a tool input too large for a check gate's environment is not delivered on stdin instead; that would change how `execute_bounded` treats an unread stdin for every caller, and `deny` and `rewrite` gates already see the whole input.
- w2-chaos D5: a hook killed with SIGKILL still leaves a running gate command behind; a pid ledger that the next hook reaps was declined as more machinery than the case is worth, and gate timeouts are the bound.
- w2-chaos D6: a Stop denied only because Yoke failed is let through the second time (`stop_hook_active`); a stricter counter that survives without the database was declined as heavier than the risk.
- w2-chaos D7: a read-only `HOME` with no `~/.yoke` still denies blocking events, because the state and the last known mode cannot be read there.

### Decisions and checks that need a person

- Confirm the exit-code policy from j7-extensions-codex-9.
  Decision 0003 was amended so that only `stale` and `dangling` findings exit 1 and a `shadowed` name exits 0.
  The revert is one line, `Drift::fails_check`.
- Verify the likely remedy for the Codex sandbox on Ubuntu 24.04, installing the distribution's `bubblewrap` package.
  The troubleshooting section marks it unverified.
- Check whether Codex keeps its hook approval after `yoke uninstall` and `yoke install`.
  No document claims it.

## How to run the next field test

1. Prepare a fresh server, ideally Ubuntu 24.04 x86_64 as before, with a non-root user, Claude Code and Codex installed and logged in, and no Yoke files.
   Record the commit or tag under test and the SHA-256 of the installed executable, because `yoke --version` only prints `0.1.0`.
2. Install with the one-line installer from the [README](../README.md#quickstart), at a terminal, and keep the output verbatim.
   Then rerun it to confirm that nothing changes.
3. Walk the seven journeys, one agent or person per journey, each in its own directory.
   Each journey writes a report with the commands and real output, then findings, then the steps only a human may run.
   Start with the steps the last test could not run, listed under "Not verified live" above.
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
