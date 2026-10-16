# Upgrade notes

These notes cover user-visible changes since the implementation baseline at commit `e8b8ee0`, for anyone replacing a Yoke binary built from it or earlier.
No release has been published yet, so they apply to development builds; see [release acceptance](release-acceptance.md).
Each item says what changed, who is affected, and what to do.

## Before you replace the binary

Let active plan checks finish before replacing the binary: this build uses stable check-ownership files in Git metadata, while older builds locked the editable plan file, so the two versions cannot coordinate a live check.

1. Replace every Yoke executable that runs hooks at the same time, including any `target/release/yoke` that installed hooks point at.
   The first writable open by the new binary migrates the state database, and an older binary then refuses it on every hook event.
2. Do not trust a configuration that uses the new `[review.models]` or `[memory.digest_models]` keys until step 1 is done.
3. After replacing, rerun `yoke install --agent claude,codex` with the same `CLAUDE_CONFIG_DIR` and `CODEX_HOME` you start the agents with, then run `yoke health`.
4. Reinstall from the same executable path the installation receipt names, or run `yoke uninstall --agent <agent>` with the new binary first.
   A reinstall from another path is refused with `executable changed; uninstall before reinstalling`, followed by both paths and the exact `uninstall` command to run; the install script does this for you.

## Storage and upgrade

### Notification project identity

- What changed: notification ownership now uses an exact private project identity rather than a display-redacted path, preventing distinct paths from sharing an inbox or acknowledgement authority.
  Public paths remain redacted.
- Who is affected: projects whose paths contain text recognized by secret redaction.
  Unambiguous old rows remain available; ambiguous old rows stay stored but hidden until an exact plan or configuration source establishes ownership.
  Decision history whose original project was already redacted cannot restore that attribution.
- What to do: use the same current binary for every hook and console reading this state; older binaries do not understand the new notification identities.
  Do not delete ambiguous rows to force recovery.

### Migrations 007 to 012 run on first open

- What changed: the state database moves from schema 6 to 12 in one transaction the first time a new binary opens it for writing, which any hook, `yoke log`, `yoke config`, `yoke jobs`, `yoke notifications` or plan command does.
  The upgrade adds plan review bases, completion fingerprints, evidence hashes, plan requirements and feedback, background jobs, notifications and usage timestamps, and rebuilds the memory search index.
  A failed migration leaves the database unchanged.
- Who is affected: everyone with an existing `~/.yoke/state.db`.
  An older binary that meets the upgraded database fails with `state database schema 12 is newer than supported`.
  Because it can then read neither state nor mode, it fails closed: it denies PreToolUse and blocks Stop and UserPromptSubmit even when the recorded mode is observe, and appends one `error.log` line per event.
  `yoke health`, `yoke statusline`, `yoke trial report`, `yoke usage` and `yoke extensions` read the database without migrating it.
- What to do: replace all hook binaries together, and keep a copy of `~/.yoke/state.db` with its `-wal` and `-shm` files if you may need to go back.
  There is no downgrade command.
  To go back, stop every agent, restore the old binary, and restore `state.db`, `state.db-wal` and `state.db-shm` together.
  A restored main file beside newer sidecars replays their frames and reads as schema 12 again.
  A copy made with `.backup` has no sidecars, so read-only commands fail with SQLite error 14 on it until the first writable open creates them.

### Configuration keys older binaries cannot read

- What changed: `[review.models]` and `[memory.digest_models]` choose the model for the reviewer and digest processes Yoke starts.
  Configuration parsing rejects unknown keys.
- Who is affected: anyone who trusts a configuration that uses these keys while an older binary still runs hooks; that binary cannot parse the trusted snapshot and fails closed.
- What to do: update every hook binary first, then add the keys and run `yoke trust`.

### Retention is bounded and covers more history

- What changed: decisions, facts and the check runs of done or cancelled plans older than `log.retention_days` are deleted at most hourly at session start, in batches with a short time budget, so a large backlog drains over several sessions.
  Check runs of cancelled plans are now pruned too; memories, trusted snapshots and the runs and evidence of unfinished plans are never pruned.
- Who is affected: long-running installations with large histories; the first sessions after the upgrade may each remove part of the backlog.
- What to do: nothing; raise `log.retention_days` first if you want to keep older history.

### Yoke refuses an unset or relative HOME

- What changed: every Yoke command and hook fails with a clear error when `HOME` is unset, empty or not an absolute path, instead of falling back to the account's home directory.
- Who is affected: scripts and services that run Yoke without `HOME`, and test harnesses that point `HOME` at a temporary directory.
- What to do: set `HOME` to an absolute directory; in enforce mode a hook without it denies blocking events.

### Every error.log line has a timestamp

- What changed: every `~/.yoke/error.log` line, including those of the background update check and job worker, now starts with a Unix timestamp.
- Who is affected: scripts that read error.log; lines written by earlier builds may still lack the timestamp.
- What to do: parse the leading integer, and treat lines without it as from an earlier build.

## Hooks and enforcement

### A completed plan reopens when its tree changes

- What changed: a done plan stays bound to its session and is accepted at Stop only while the working tree content matches the content its checks passed on.
  Committing exactly the checked bytes keeps it done, because the comparison ignores HEAD and the index.
  The comparison reads paths, modes and commit ids from git and hashes files from disk, so submodules, nested repositories and large binary files work; a tree that cannot be compared counts as changed instead of failing every Stop.
  A file being written or removed while Yoke hashes it also counts as changed rather than as an error.
  After a later edit, the next Stop in enforce mode reopens it as `building` and blocks until `yoke plan check` passes again; observe mode only records a would-be denial.
  Standalone `yoke review` evidence no longer refreshes a completed plan.
- Who is affected: enforce-mode sessions that keep working after their plan is done.
  A plan completed before the upgrade has no content anchor, so its next Stop in enforce mode reopens it once and one fresh `yoke plan check` completes it again.
- What to do: start a new plan for follow-up work, or rerun `yoke plan check <slug>`.

### Plan enforcement follows the plan's project

- What changed: a session's latest event sets its recorded project, and its active plan is enforced whenever the session stops in the plan's own project, even after it visited another project.
  Binding a plan requires the session's latest event to be in that plan's project, and a session still owns at most one unfinished plan.
- Who is affected: sessions that move between projects.
- What to do: run `yoke plan start` from the plan's project.

### Blocking hooks answer within a deadline

- What changed: a blocking hook that has not decided by its deadline denies in enforce mode, prints nothing in observe mode, records the overrun as a decision and in `~/.yoke/error.log`, and kills the gates still running.
  The deadline is 25 seconds for UserPromptSubmit and, for PreToolUse and Stop, 120 seconds or the sum of that event's check gate timeouts plus 10 seconds, whichever is longer, up to 540 seconds.
  A Stop the watchdog denies counts toward `stop.max_blocks`, so the plan stalls and the agent can stop.
  Agents let an action through when a hook outlives their own timeout, so this keeps a slow hook from failing open.
- Who is affected: policies with slow gates.
  `yoke trust` now refuses check gates whose timeouts on one event add up to more than 530 seconds for PreToolUse or Stop, or 20 seconds for UserPromptSubmit; a UserPromptSubmit check gate therefore needs an explicit `timeout` of 20 or less.
- What to do: lower those gate timeouts before trusting; policies trusted earlier still load, and their gates are cut at the deadline.

### Plan context is advisory

- What changed: if the session's plan cannot be read when SessionStart or UserPromptSubmit adds plan context, Yoke warns and leaves the context out instead of failing the event.
- Who is affected: sessions on read-only state at an older schema.
- What to do: nothing.

### Read-only state keeps allowed stops

- What changed: when state cannot be written, a Stop with no plan state to update is allowed with a warning.
  A building or checking plan, a completed plan whose tree changed, or plan state that cannot be read still blocks in enforce mode.
- Who is affected: sessions on a full disk or read-only state directory.
- What to do: free disk space or repair permissions in your own terminal.

### Usage accounting never decides a hook event

- What changed: a transcript over 32 MiB or otherwise unreadable now only warns on Stop, SessionEnd and PreCompact, instead of recording SessionEnd and PreCompact as `error` decisions.
  An unterminated last line is left for the next import, because agents append transcripts asynchronously.
  There is no per-line size limit: large tool results and assistant messages are parsed for accounting fields without retaining their content in the decoded record, so large usage-bearing messages are counted too.
- Who is affected: sessions beyond the whole-file limit or with unreadable input; their usage is still not imported, and each import logs `usage ingestion failed`.
- What to do: nothing.

### Codex counter restarts no longer freeze a session's usage

- What changed: a Codex rollout whose cumulative token total restarts when Codex begins a new conversation segment, for example after a thread settings change, used to fail every later import of that session with `Codex cumulative counters decreased`, so its usage stayed frozen at the last import before the restart.
  A restart now starts a new accounting segment: the samples recorded before it stay, the usage since the restart is added, and importing the same transcript again yields the same samples.
  A restart is recognised when the total decreases, or when the total equals the last turn's usage while an earlier total existed.
- Who is affected: long Codex sessions; their usage catches up on the next Stop, SessionEnd or PreCompact.
- What to do: nothing.
  A restart that neither decreases the total nor is reported as the last turn's usage cannot be told apart from ordinary growth, so only the difference from the previous total is counted.

### New human-only commands

- What changed: `plan cancel`, `plan reassign`, `plan accept`, `plan resolve`, `plan acknowledge`, `jobs retry` and `notifications ack` require a human in an interactive terminal, and the built-in guard denies agent tool calls that run any of the `plan` ones.
- Who is affected: automation that tried to run these from an agent.
- What to do: run them yourself in your own terminal.

## Installation and health

### Installation follows the agents' configuration directories

- What changed: `yoke install`, `uninstall`, `health` and `extensions` use `CLAUDE_CONFIG_DIR` and `CODEX_HOME` when they are set (they must be absolute), and `~/.claude` and `~/.codex` otherwise.
- Who is affected: anyone who starts Claude Code or Codex with either variable; earlier builds wrote hooks to files those agents never read.
- What to do: with the variables unset, run `yoke uninstall` to remove hooks from the old location, then run `yoke install` with the variables set.
  An installation receipt for a different settings path is refused with `settings path differs from installation receipt`.

### Installation reports settings that disable hooks

- What changed: a first install refuses while Claude user settings set `disableAllHooks`, or file-based managed settings set `disableAllHooks` or `allowManagedHooksOnly`; a reinstall reports them as warnings and never edits them.
  `allowManagedHooksOnly` in user settings is no longer checked, because Claude Code applies it only from managed settings.
- Who is affected: machines with those settings.
- What to do: change the setting yourself, or ask whoever manages it, then reinstall.

### Claude installs a PostToolUseFailure hook

- What changed: Claude Code now gets a `PostToolUseFailure` hook, and failed `Bash` and `PowerShell` commands become memory facts only through it.
  Codex documents no failure event, so its command failures are not recorded.
- Who is affected: existing Claude installations, which lack the hook until reinstalled.
- What to do: run `yoke install --agent claude`; reinstalling also labels older Yoke hooks with `Yoke: <event>` without duplicating them.

### `yoke health` is read-only and exits 1 on failure

- What changed: `yoke health [--json]` reports installation, hook enablement, last event per agent, trusted policy, mode and unfinished plans, and never creates, migrates or writes anything.
  It exits 1 when no agent is installed, installed hooks cannot run, or the state database cannot be read; observe mode, untrusted policy and missing plans are guidance with exit 0.
- Who is affected: anyone scripting it.
- What to do: treat exit 1 as "protection is not active".

## Plans and review

### `yoke plan status` prints text by default

- What changed: `yoke plan status <slug>` prints a text summary; add `--json` for the full record, which earlier builds printed by default.
- Who is affected: scripts that parse its output.
- What to do: add `--json`.

### Plans started before the upgrade review against HEAD

- What changed: a plan now pins the commit it started from, and independent review sees work committed since then.
  A plan started by an older binary has no recorded review base, so its review still diffs the working tree against `HEAD` and does not see work committed after the plan started.
- Who is affected: plans that were started before the upgrade and are still unfinished.
- What to do: finish or cancel such plans, or start a fresh plan, if committed work must be reviewed.

### Plans can declare requirements and tasks

- What changed: optional `[[requirement]]` and `[[task]]` header sections add an `acceptance` check run, and a plan with them is done only when every requirement is satisfied and every task is complete.
  A check named `acceptance` is reserved when either section is present; plans without them behave as before.
- Who is affected: authors of new plan headers.
- What to do: see [workflows](workflows.md).

### Open blocking validation findings hold approval

- What changed: `yoke plan approve` is refused while a blocking finding from `yoke plan validate` is open, including findings from earlier revisions of the plan.
- Who is affected: plans that were validated.
- What to do: close each finding with `yoke plan resolve` or `yoke plan acknowledge`.

### Review uses the plan's start commit

- What changed: plans started after the upgrade record the `HEAD` commit, and independent review diffs against it, so work committed during the plan stays in review.
  Plans started earlier fall back to `git diff HEAD`, and review evidence names the reviewer agent and requested model.
- Who is affected: plans with commits made during the work.
- What to do: nothing.

### Interrupted checks and evidence

- What changed: `yoke plan resume` recovers a plan stuck in `checking` once no live process owns the check, and the console offers the same recovery.
  Screenshots captured by checks are hashed, and status reports each as `verified`, `tampered`, `missing`, or `unrecorded` for runs recorded before the upgrade.
- Who is affected: plans with artifacts or killed checks.
- What to do: nothing.

### `yoke plan check` needs advisory file locks

- What changed: a running check holds a persistent, private per-plan ownership file in Git metadata, so atomically saving the plan cannot replace its lock.
  `yoke plan check` refuses to run when advisory locks are unavailable or Git metadata cannot be written.
- Who is affected: projects on some network filesystems or with read-only Git metadata.
- What to do: keep Git metadata on a writable filesystem that supports advisory locking, and never delete ownership files while a check may be running.

## Memory and usage

### Recall matches any term

- What changed: `yoke recall` and the console's memory search match any query term, rank results by relevance (BM25) before pinned status, and match word stems.
  An empty query still lists pinned memories first, then the newest.
- Who is affected: anyone relying on the old all-terms matching.
- What to do: add distinctive terms to narrow a search; `yoke recall --explain` shows why each memory matched.

### Digests run as background jobs

- What changed: memory digests are queued as durable jobs and run by a background `yoke jobs work` process that SessionStart may start; failed jobs retry with backoff and become `dead` after three attempts.
- Who is affected: projects with `memory.digest` enabled.
- What to do: inspect them with `yoke jobs`, and retry a dead job with `yoke jobs retry <id>` in your own terminal.

### Killed sessions are digested at the next session start

- What changed: facts of a session whose end-of-session hook never ran, or ran without queuing a digest, are no longer left pending.
  The next session start in the same project queues a digest for every session with pending facts and no recorded activity for 30 minutes, oldest first and at most 20 sessions per start.
  The first session start after this upgrade can therefore queue digests for earlier sessions whose facts were never digested, and each worker run handles at most 5 of them.
- Who is affected: projects with `memory.digest` enabled.
- What to do: nothing; `yoke jobs` lists the queued digests, and a dead one still waits for `yoke jobs retry <id>`.

### Usage samples carry a time

- What changed: imported usage samples now record their time, and the console's Usage page filters and groups them by day or month in an explicit UTC offset.
  Samples imported before the upgrade have no time until their session's transcript is imported again, and the page counts them as undated.
- Who is affected: anyone reading historical usage.
- What to do: nothing; usage of ended sessions stays undated.

### Redaction no longer leaves a stray bracket

- What changed: values that were already masked, such as `--token sk-...`, now read `[REDACTED]` instead of `[REDACTED]]`, and any unmasked tail is masked too.
  A secret key inside a masked value, as in `--token sk-a/password b`, now has its own value masked too, and `bearer` keeps its original case.
  A quoted secret value whose quote never closes, as in `password="abc def`, is masked to the end of the text.
- Who is affected: searches over stored decisions and facts.
- What to do: nothing; rows stored earlier keep their old text.

## Console and new commands

### Console reads take a brief write lock

- What changed: each console page render and each `yoke notifications` run synchronizes notifications in a short write transaction, which writes nothing when nothing changed.
  Hooks that arrive during it wait up to their two-second busy timeout.
- Who is affected: very busy sessions with the console open.
- What to do: nothing.

### New pages and commands

- What changed: the console adds Projects, Notifications and Workflow settings pages, and an unknown `project` parameter now returns HTTP 400.
  New commands are `yoke extensions` (read-only inventory that exits 1 on stale or dangling definitions), `yoke notifications`, `yoke jobs`, `yoke trial report`, `yoke health`, `yoke plan diff`, `yoke plan validate` and the plan task, comment and feedback commands.
- Who is affected: everyone.
- What to do: run `yoke --help` for the list; [CLI reference](cli.md) has details.
