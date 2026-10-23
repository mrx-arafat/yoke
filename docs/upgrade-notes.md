# Upgrade notes

### Schema 15 keeps exact usage summaries

- What changed: writable opens upgrade schemas 12 through 14 to 15 in one transaction.
  The upgrade builds exact session, model and UTC-day summaries, adds time indexes and preserves raw usage samples and existing notification acknowledgements.
  New partial transcript imports raise a `usage_partial` notification; a complete replacement import resolves it.
  Existing partial sessions are backfilled in bounded batches when notifications are read.
- Who is affected: installations with existing state databases.
  The initial summary build reads existing usage once while holding the write lock; its duration depends on the amount of usage history.
  Ordinary opens do not rebuild summaries or run statistics maintenance.
  Read-only commands can still inspect schemas 12 through 14 with their older query paths.
- What to do: replace all hook binaries together because older writers refuse schema 15.
  Keep a backup of the database and its sidecars before upgrading if you may need to restore an older binary.
  There is no downgrade command; raw usage samples are retained for detailed history.

### Preset and shared-knowledge locks recover when a writer exits

- What changed: preset writes record their PID in a persistent advisory lock file.
  Shared knowledge uses a persistent `knowledge.lock` in the project's `.yoke` directory.
  These files may remain after success or a crash; an existing file alone no longer means a writer holds the lock.
- What to do: retry after a live writer finishes instead of deleting its lock file.
  Existing shared note entries remain recognized, while new entries indent their content to keep marker-shaped note text separate from entry markers.

Memory mutation reports now default to readable text.
Scripts reading JSON from `yoke remember`, `yoke memory export` or `yoke memory import` must add `--json`; their JSON report fields and epoch timestamps are preserved.
Plan list and status output now include the bound session; JSON uses `null` when no session is bound.
Automatic release checks can be disabled with `[update]` and `check = false` in trusted configuration, followed by `yoke trust` in your own terminal (`--global` for global configuration).
Explicit `yoke update --check` remains available.

Project directory names must be UTF-8 for project plans, memory and accounting.
If a directory name is not UTF-8, hooks report `project scope unavailable` and continue applying trusted parent and global policy instead of denying every tool call because the path cannot be stored.
Rename the directory using UTF-8 names and restart the agent there to restore project scope.

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

### Hook installation records and process boundaries

- What changed: new ownership receipts keep only Yoke's entries and hashes instead of complete agent settings.
  Reinstalling upgrades existing receipts; uninstall still accepts older receipts.
  The five newest private settings backups are retained, can contain credentials, and survive uninstall.
  Exact byte restoration uses a retained original backup; when it expired, uninstall removes owned entries from the current file.
  Dry runs omit unrelated settings and hide statusline commands.
  Install now canonicalizes the executable and requires a regular executable file; install and health warn about shared-writable locations and build directories.
  Git subprocesses use an absolute executable, disable filesystem monitoring and hooks, and discard inherited `GIT_*` overrides.
  Agent tool calls cannot invoke `run-hook`, and valid recorded payloads gain OS-derived `_yoke_parent` audit context without a database schema change.
- Who is affected: existing development installs and scripts relying on Git environment overrides or complete dry-run context.
- What to do: run `yoke install` from your own terminal to upgrade the receipt, then `yoke health`.
  Move development executables to a private stable location, uninstall the old hooks and reinstall from that location.
  Copy recovery backups you need before the next settings changes expire them, and delete backups containing credentials once recovery is no longer needed.
  Stop other settings writers while installing; if Yoke reports a concurrent save, compare the named recovery file with the settings before retrying.
  The persistent `install.lock` file names the last holder; an OS lock releases automatically after a crash, so do not delete that file to unblock an installer.

### Memory scope, context limits and moved project directories

- What changed: agent memory commands now require a hook-recorded session anchor whose parent process identity matches the tool's OS ancestry.
  A directory change no longer changes the memory project, and an unavailable or ambiguous binding refuses the command.
  Agent exports cannot include all projects, and import previews cannot map records into another project.
  Pinning and real imports remain human-only.
  An injected item uses at most one quarter of the character budget, pins together use at most half, and space remains for approved knowledge and unpinned notes.
  `yoke project move <old> <new> --dry-run` previews a transactional move of stored project attribution after its directory moves.
  The real command requires a human terminal, and an occupied destination requires an explicit `--merge`.
  Changed or missing destination trust snapshots drop the source trust batch; merging replaces any destination project trust with the verified source batch.
- Who is affected: active sessions started with older hooks, oversized pinned notes, and people renaming or relocating a project directory.
- What to do: update the binary your hooks run and restart agents in the project before using agent memory commands.
  Run the command in your own terminal if your harness does not expose a unique parent binding.
  Split oversized notes for useful context; complete text remains available through `yoke recall`.
  Stop agents before moving a directory, inspect `project move --dry-run`, and use the exact absolute old path recorded by Yoke.
  If plan slugs collide, choose a destination without those plans or keep the histories separate.
  Restart sessions at the new location and run `yoke trust` yourself when the move reports `untrusted`.
  Project paths already redacted in historical records are refused because their old attribution cannot be recovered unambiguously.

### State files are private and the log keeps less of a tool's output

- What changed: `~/.yoke` is now 0700 and `state.db`, its `-wal` and `-shm` files, `error.log` and the install backups are 0600; an existing installation is tightened the next time Yoke opens its database.
  Redaction also masks more credential shapes (GitHub user and refresh tokens, Stripe live keys, Google API keys and OAuth tokens, newer Slack tokens, GitLab, npm, Hugging Face, SendGrid, Shopify and DigitalOcean tokens, JSON web tokens, passphrases, `DB_PASS`-style names, cookies and session ids, Azure `AccountKey`, signed-URL `sig` and `X-Amz-Signature`, and a `"auth"` field).
  The decision log stores the size and status flags of a tool's `tool_response` instead of its content, with a 200-character excerpt of a shell command's error output, and cuts a failed tool's `error` at 256 characters.
- Who is affected: anyone who shares the machine with other users, or who read `tool_response` from `yoke log --json`.
- What to do: nothing for the modes.
  Rows stored by earlier versions keep their old content until retention (`log.retention_days`) removes them; run `yoke forget` for notes that hold a credential.
  Read `tool_response.excerpt` where you used to read the whole response.

### Injected notes are fenced, and pins and imports need a person

- What changed: session start injects notes inside a `<yoke-memory trust="untrusted-data">` block that labels each item and says the text is data, not instructions.
  `yoke remember --pinned`, `yoke memory pin` and the real `yoke memory import` refuse to run from an agent's tool process.
  An import unpins every note unless `--keep-pins` is given, stores pending facts as already digested, rejects out-of-range timestamps, and a dry run prints the text it would add.
- Who is affected: anyone who scripts these commands from inside an agent, or restores pinned notes from an export.
- What to do: run them in your own terminal, and add `--keep-pins` when restoring your own export.
  Notes pinned by an agent before this change stay pinned; review them with `yoke recall` and unpin what you did not write.

### Redaction masks URL passwords and leaves prose readable

- What changed: redaction now masks passwords in URLs such as `scheme://user:PASSWORD@host`, masks the whole value of a `key: value` line, and no longer masks ordinary words after "token", "secret" or "password" or names such as `ask-codex`.
- Who is affected: anyone who stored notes or ran hooks with credentials in URLs before this change.
- What to do: rows stored by earlier versions keep their original text, so review old notes with `yoke recall` and run `yoke forget <id>` on any that hold a credential.

### Notification project identity

- What changed: notification ownership now uses an exact private project identity rather than a display-redacted path, preventing distinct paths from sharing an inbox or acknowledgement authority.
  Public paths remain redacted.
- Who is affected: projects whose paths contain text recognized by secret redaction.
  Unambiguous old rows remain available; ambiguous old rows stay stored but hidden until an exact plan or configuration source establishes ownership.
  Decision history whose original project was already redacted cannot restore that attribution.
- What to do: use the same current binary for every hook and console reading this state; older binaries do not understand the new notification identities.
  Do not delete ambiguous rows to force recovery.

### A session keeps the project it started in, and gates run in the project

- What changed: a hook now treats the project as fixed where the session starts (`CLAUDE_PROJECT_DIR` for Claude Code, else the directory of the session's first event) and remembers it in a new `session_anchors` table, schema 14.
  The directory a later event reports only selects a nested `yoke.toml` that a human trusted, so a directory outside the project, a symlink that leaves it, or a `yoke.toml` nobody trusted no longer turns the project's trusted policy off.
  Check gates run in the directory of the trusted `yoke.toml` instead of the directory the agent reports; `YOKE_CWD` still carries the reported directory.
- Who is affected: everyone with gates whose commands are relative and were written for the directory the agent worked in, and anyone who relied on an agent that left the project being governed by another policy.
  The decision log, memory and plans of a session follow the project of its start directory, not the directory each tool call reports.
- What to do: write gate commands relative to the `yoke.toml` that defines them, and start the agent in the project.
  An older binary refuses a database at schema 14 as described under migrations below, so replace all hook binaries together.

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

### Migration 013 builds indexes, which takes seconds on a large history

- What changed: schema 13 adds three indexes: the newest decision of an agent (`yoke health`), a covering index for `yoke trial report`, and fact timestamps for retention.
  They make those commands and the retention scan independent of the size of the history.
  The same release stops sending log filters you did not give to SQLite, so `yoke log --tool`, `--outcome`, `--session` and `--project` use the indexes too.
- Who is affected: everyone with an existing state database.
  The build is proportional to history: about one second per 50 thousand decisions with a warm cache, and 19 to 26 seconds for 500 thousand decisions and 200 thousand facts on a cold cache.
  The database grows by about 6 percent.
  A new installation pays nothing.
- While it runs, the hook or command that started it holds the write lock.
  Other hooks wait 2 seconds, then decide from read-only state: gates still apply and a deny is still a deny, but those events are not recorded, and each of them costs 2 seconds.
- What to do: after replacing the binary, close your agents and run `yoke log --limit 1` once in your own terminal, so the build does not delay an agent's tool calls.
  A database you migrated cannot be opened by the old binary; to go back, restore the copy you kept as described above.

### Configuration keys older binaries cannot read

- What changed: `[review.models]` and `[memory.digest_models]` choose the model for the reviewer and digest processes Yoke starts.
  Configuration parsing rejects unknown keys.
- Who is affected: anyone who trusts a configuration that uses these keys while an older binary still runs hooks; that binary cannot parse the trusted snapshot and fails closed.
- What to do: update every hook binary first, then add the keys and run `yoke trust`.

### Retention is bounded and covers more history

- What changed: decisions, facts and the check runs of done or cancelled plans older than `log.retention_days` are deleted by a background job.
  Session start queues the job at most hourly and returns at once; a worker deletes in short batches with pauses so hooks are not held up, and queues a follow-up pass when a large backlog does not fit one pass.
  Check runs of cancelled plans are now pruned too; memories, trusted snapshots and the runs and evidence of unfinished plans are never pruned.
- Who is affected: long-running installations with large histories; the first session after the upgrade queues the cleanup of the whole backlog, which then runs in the background for seconds to minutes and shows as the `retention` job in `yoke jobs --all`.
- What to do: nothing; a failing cleanup shows in `yoke health` and `yoke jobs`, and raise `log.retention_days` first if you want to keep older history.

### Yoke refuses an unset or relative HOME

- What changed: every Yoke command and hook fails with a clear error when `HOME` is unset, empty or not an absolute path, instead of falling back to the account's home directory.
- Who is affected: scripts and services that run Yoke without `HOME`, and test harnesses that point `HOME` at a temporary directory.
- What to do: set `HOME` to an absolute directory; in enforce mode a hook without it denies blocking events.

### Every error.log line has a timestamp

- What changed: every `~/.yoke/error.log` line, including those of the background update check and job worker, now starts with a Unix timestamp.
- Who is affected: scripts that read error.log; lines written by earlier builds may still lack the timestamp.
- What to do: parse the leading integer, and treat lines without it as from an earlier build.

## Hooks and enforcement

### Switching Yoke off needs a person, and settings files cannot do it

- What changed: `YOKE_OFF=1` no longer skips a hook.
  A hook skips only when `YOKE_OFF` equals the private token that `yoke pause` prints (kept in `~/.yoke/off-token`, mode 0600), or while a pause set with `yoke pause --for 30m` lasts; every honoured or refused skip is written to `~/.yoke/error.log`.
  The guard also denies agent writes, edits and patches to Claude Code's `settings*.json`, Codex's `hooks.json` and `config.toml`, a project `.claude` or `.codex`, and the `yoke` executable, and it denies reading the token.
  `yoke install`, `yoke uninstall` and `yoke update` (without `--dry-run` or `--check`) refuse to run from an agent's tool process, and `yoke pause` is human-only.
  `yoke health` and the statusline report `env.YOKE_OFF` or `env.HOME` in a Claude settings file, an active pause, and a session that is open but has recorded no hook decision for 20 minutes.
- Who is affected: scripts and wrappers that set `YOKE_OFF=1` to start an agent without Yoke, and agents that were asked to edit their own hook settings or to reinstall Yoke.
  Yoke's own review and digest children still pass `YOKE_OFF=1`, which hooks now refuse; what keeps their hooks from running is that they start with the agent's settings turned off (`--setting-sources ""` for Claude Code, `--ignore-user-config` for Codex), which was read from the code and not tested with a live Codex.
- What to do: run `yoke pause` in your own terminal and use the token it prints, or pause for a limited time; run `yoke install` and `yoke uninstall` yourself.

### Hooks ignore a HOME that is not the account's home

- What changed: a hook of a release build that finds `HOME` different from the home directory in the user database uses the account's home when that holds Yoke state, and says so once on stderr and in `~/.yoke/error.log`.
  A project settings file can set `HOME` for every hook it starts, which would otherwise point Yoke at an empty state with no trusted policy.
  Debug builds and builds made with `--features honor-home-env` keep honouring `HOME`, and an unset or relative `HOME` is still an error in every build.
- Who is affected: anyone who deliberately starts an agent with another `HOME` against an optimized release binary, such as an isolated acceptance run.
- What to do: build with `--features honor-home-env` for such runs (see [CONTRIBUTING](../CONTRIBUTING.md#testing-against-real-state)), or run the agent with its real `HOME`.

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

### Setup help and policy recovery guidance

- What changed: top-level help shows the setup sequence, and `init`, `trust`, `install` and `health` help include examples and constraints.
  Health uses the correct trust command for stale global or project policy, asks you to restore missing trusted files, names the policy supplying observe mode, and gives launch and verification commands when an agent has no recorded event.
- Who is affected: anyone setting up or diagnosing an installation; report fields and exit codes are unchanged.
- What to do: run `yoke health` in the project and follow its guidance from your own terminal.

### The installer builds one exact revision, and the updater tests what it downloads

- What changed: when `install.sh` builds from source it uses `YOKE_COMMIT`, `YOKE_VERSION`, the newest release tag, or, while no release exists, the commit `main` points to when the installer runs; it looks that hash up, prints it and builds exactly it (so a push to `main` mid-install cannot change what is built), and it stops, with the `YOKE_COMMIT=` command to run, when it cannot look the hash up.
  It rejects a plain `http://` `YOKE_DOWNLOAD_URL` unless the host is `localhost`, `127.0.0.1` or `[::1]`, refuses an install directory that contains a double quote, dollar sign, backtick, backslash or newline, and limits downloads to TLS 1.2 or newer, ten minutes and 64 MiB.
  `yoke update` runs curl from a fixed system path when there is one, asks for TLS 1.2 or newer, and runs the downloaded executable once with `--version` before it replaces the installed one.
  Release archives no longer carry the builder's owner or file times, and releases gain a `SHA256SUMS` asset and signed build provenance (see [Verifying releases](verifying-releases.md)).
- Who is affected: anyone who installs while no release exists and expected the newest `main`, and anyone with an install directory or mirror URL of the kinds above.
- What to do: to build another commit, set `YOKE_COMMIT` to its full hash.
  Use an `https://` mirror, or a loopback address for a local one.

### The digest worker reads only transcripts

- What changed: the background digest reads a transcript only when its path, with symlinks and `..` resolved, is a `.jsonl` file inside the job's project or an agent's transcript directory (`$CLAUDE_CONFIG_DIR/projects` or `~/.claude/projects`, and `$CODEX_HOME/sessions` or `~/.codex/sessions`).
  Any other path is ignored with a line in `~/.yoke/error.log`, and the digest runs from the recorded facts alone.
  A new worker `error.log` is created with mode 0600.
- Who is affected: anyone who keeps agent transcripts somewhere else.
- What to do: nothing for the default locations; `yoke memory digest --transcript <file>` in your own terminal still reads the file you name.

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

### Check gates and Codex rewrites

- What changed: check gate children receive a reduced environment and have core dumps disabled.
  They keep the documented `YOKE_*` variables, `HOME`, `PATH`, terminal and locale settings, toolchain locations such as `CARGO_HOME`, `RUSTUP_HOME` and `VIRTUAL_ENV`, and certificate paths listed in [Gate environment](configuration.md#gate-environment).
  CPU, file-size and descriptor limits stay inherited, and the existing wall-clock process-group timeout remains in force.
  Other inherited variables, including credentials and proxy URLs, are removed unless explicitly passed by name.
  Large `YOKE_TOOL_INPUT` values are shortened to 32 KiB with `[YOKE TOOL INPUT TRUNCATED]` and `YOKE_TOOL_INPUT_TRUNCATED=1`.
- Who is affected: existing `yoke.toml` checks that depend on additional inherited variables or complete large JSON inputs, and Codex rewrite gates without narrow matchers.
- What to do: add any required inherited names to `[gates] pass_env = ["DATABASE_URL"]` or a check gate's `pass_env = ["SERVICE_TOKEN"]`, then review and run `yoke trust` in your own terminal.
  A failing gate lists withheld names without values and points to `pass_env`.
  Check `YOKE_TOOL_INPUT_TRUNCATED` before parsing input.
  For a Codex rewrite gate, add both a nonempty `match_tool` and a nonempty `match_contains`, then review and run `yoke trust` in your own terminal.
  Otherwise that gate reports `Codex rewrite gate '<name>' requires nonempty match_tool and match_contains; add both matchers to yoke.toml and approve the updated policy with yoke trust` and enforce mode refuses calls for its event; observe mode records the fault and continues without rewriting.
  Existing Claude rewrite policies keep their behavior.
  Plan checks may now declare `inputs = ["scripts/check.sh"]` to display and freeze SHA-256 hashes at approval; checks without `inputs` and existing database schemas require no migration for this feature.

### New pages and commands

- What changed: the console adds Projects, Notifications and Workflow settings pages, and an unknown `project` parameter now returns HTTP 400.
  New commands are `yoke extensions` (read-only inventory that exits 1 on stale or dangling definitions), `yoke notifications`, `yoke jobs`, `yoke trial report`, `yoke health`, `yoke plan diff`, `yoke plan validate` and the plan task, comment and feedback commands.
- Who is affected: everyone.
- What to do: run `yoke --help` for the list; [CLI reference](cli.md) has details.
