# Yoke design: gates, workflow, memory, console, presets

Status: draft for review.
Date: 2026-10-13.
Scope: everything from 0.1.0 (gate skeleton) to 1.0.

## 1. Goal

Yoke replaces a heavier agent harness on the maintainer's Mac for daily work, and ships publicly so anyone can `cargo install` or download it.
Daily use is the test bed: a feature is done when it survives a working day, not when its unit tests pass.

Success for 1.0:

- Claude Code and Codex both run under yoke in `enforce` mode with no other harness installed.
- A plan cannot reach `done` without yoke itself having run its checks on the final working tree.
- Memory from past sessions shows up at session start within a fixed token budget.
- No yoke process outlives the command or hook that started it, except `yoke console` while it is open.

## 2. Principles

1. **Code enforces, prompts inform.** Anything that must hold is checked by yoke's binary. Text injected into the agent only explains what to do next.
2. **Fail closed, never silently.** An internal yoke error on a blocking event denies the action and shows the error. Every error is logged. Nothing is swallowed.
3. **One owner per fact.** Each piece of state has exactly one source of truth. Copies are derived and never written back.
4. **Observe before enforce.** Every install starts in `observe` mode: yoke logs what it would do and blocks nothing.
5. **Local only.** No telemetry, no sharing service, no remote config. The only network calls are `yoke update` and its daily check.
6. **No daemon.** Hooks are short processes. Background work runs as a detached child that exits when done.
7. **Not a sandbox.** Yoke stops agent mistakes and casual self-unblocking. An agent with shell access and bad intent can bypass any hook; containing that needs OS sandboxing, which is out of scope.

## 3. Architecture

### 3.1 Processes

| Process | Lifetime | Started by |
|---------|----------|------------|
| `yoke run-hook` | One hook event, target under 50 ms excluding gate commands | The agent |
| `yoke statusline` | One refresh, target under 15 ms | The agent |
| `yoke jobs work` (memory digests), `yoke update --check` | Until done, detached | A hook |
| `yoke console` | While the user keeps it open | The user |
| Everything else (`plan`, `trust`, `recall`, ...) | One command | The user or the agent |

Every child process yoke spawns that runs an agent headless (digest, review) gets `YOKE_OFF=1`, so the child's own hooks do not recurse into yoke.

### 3.2 Crates and modules

- `yoke-core`: all logic, no I/O beyond files, SQLite and child processes.
  - `event`: event kinds and payload parsing.
  - `wire`: the hook output JSON, shared by Claude Code and Codex.
  - `config`: config model, layering, origin tracking.
  - `trust`: trusted config snapshots.
  - `store`: SQLite connection, migrations, queries.
  - `redact`: secret masking for logged payloads.
  - `engine`: gate evaluation, built-in guards, decisions.
  - `plan`: plan files, state machine, checks, fingerprints.
  - `guard`: the stop guard.
  - `memory`: facts, memories, recall, digest input.
  - `review`: headless reviewer runs and findings parsing.
  - `usage`: transcript token counting and pricing.
  - `preset`: bundled gate packs.
- `yoke-cli`: argument parsing, command handlers, install and update, the console server.

Files stay under 800 lines; a module that grows past that splits.

### 3.3 State

One SQLite database at `~/.yoke/state.db`, in WAL mode with a 2 second busy timeout.
Migrations are numbered and tracked with `PRAGMA user_version`.
Every row that belongs to a project carries the project root path; every row that belongs to a session carries the session id.

Files outside the database:

| Path | Owner | Purpose |
|------|-------|---------|
| `~/.yoke/config.toml` | User | Global config (runs only from its trusted snapshot) |
| `<repo>/yoke.toml` | Project | Project config (runs only from its trusted snapshot) |
| `<repo>/.yoke/plans/<slug>.md` | Project | Plan content, committed to git |
| `<repo>/.yoke/knowledge.md` | Project | Shared knowledge, committed to git |
| `~/.yoke/backups/` | Yoke | Agent settings backups made by `yoke install` |

### 3.4 Data model

| Table | Key columns | Notes |
|-------|-------------|-------|
| `trust` | `scope`, `root`, `file`, `sha256`, `content`, `trusted_at` | One row per trusted file; `content` is the snapshot yoke runs |
| `sessions` | `id`, `agent`, `project`, `started_at`, `ended_at`, `active_plan` | Created on first event |
| `decisions` | `ts`, `session_id`, `agent`, `project`, `event`, `tool`, `mode`, `outcome`, `reason`, `gates`, `payload` | `outcome` is `allow`, `deny`, `would_deny`, `rewrite` or `error`; `gates` is JSON of name, result, milliseconds |
| `facts` | `ts`, `session_id`, `project`, `kind`, `detail`, `digested` | Recorded by hooks, no model |
| `memories` | `id`, `ts`, `project`, `source`, `text`, `pinned`, `shared` | `source` is `fact`, `remember` or `digest` |
| `memories_fts` | FTS5 over `memories.text` | External-content table kept in sync by triggers; `porter unicode61` tokenizer |
| `jobs` | `kind`, `dedupe_key`, `project`, `payload`, `state`, `attempts`, `max_attempts`, `run_after`, `owner_pid`, `lease_expires`, `last_error` | Durable background work; `state` is `queued`, `running`, `succeeded`, `failed` (retry scheduled) or `dead`; one active job per `dedupe_key` |
| `plans` | `id`, `project`, `slug`, `file`, `state`, `approved_header`, `base_ref`, `done_fingerprint` | `approved_header` is the frozen header snapshot; `base_ref` is the `HEAD` commit recorded when the plan started; `done_fingerprint` is the working tree the plan's checks passed on, set only when Yoke marks it done |
| `transitions` | `plan_id`, `ts`, `from`, `to`, `actor`, `note` | `actor` is `human` or `agent` |
| `check_runs` | `plan_id`, `ts`, `name`, `command`, `exit_code`, `output_tail`, `fingerprint`, `blocking`, `ms` | One row per check per run |
| `stop_guard` | `session_id`, `plan_id`, `blocks`, `last_fingerprint`, `last_block_at` | Consecutive blocks without progress |
| `plan_completions` | `plan_id`, `kind`, `item`, `definition`, `ts`, `actor`, `evidence` | `kind` is `task` or `requirement`; `definition` is the SHA-256 of the approved item, so evidence for an edited item no longer counts |
| `plan_feedback` | `plan_id`, `parent_id`, `author`, `severity`, `anchor`, `digest`, `text`, `state`, `resolved_by`, `resolved_at`, `note` | Validation findings (`severity` `blocking` or `advice`), comments and replies; `digest` is the SHA-256 of the plan text the item was written against |
| `notifications` | `key`, `kind`, `project`, `subject`, `plan_id`, `title`, `detail`, `count`, `first_seen`, `last_seen`, `state`, `acknowledged_at`, `actor`, `resolved_at` | Derived when listed from decisions, plans, check runs and trust snapshots; one row per dedupe `key`; `state` is `unread` or `acknowledged`, `actor` is `human` or `yoke` (resolved before a human saw it); empty `project` means the global config |
| `kv` | `key`, `value` | Small values such as the last update check |

## 4. Sub-project 1: Core

### 4.1 Hook flow

`yoke run-hook --agent <claude|codex> <Event>`:

1. If `YOKE_OFF=1` is set in the hook's environment, exit 0 with no output.
2. Read the JSON payload from stdin and normalize it into an `AgentEvent`.
3. Load config from trusted snapshots only (section 4.4).
4. Run the built-in self-protection guard (section 4.5), then user gates in config order.
5. Write one `decisions` row.
6. In `enforce` mode, print the result in the wire format (section 4.2). In `observe` mode, print nothing; a deny is logged as `would_deny`.

If writable database initialization fails, Yoke attempts to read the existing trusted snapshots using a live read-only database connection.
Audit, memory-fact, retention, and update-check failures are advisory and preserve the evaluated tool decision.
A Stop whose atomic record fails keeps its evaluated decision with a warning only when the stop guard needs no plan-state write; with a building or checking plan, a stale completed plan, or unreadable plan state the failure is an `error`.
Policy evaluation failures, unreadable trusted state, and failed atomic Stop updates produce an `error`; blocking events deny in `enforce` mode or when the mode cannot be determined.
On other events and in `observe` mode, errors are reported without blocking.
Errors are printed to stderr and, where possible, recorded in the database or appended to `~/.yoke/error.log`.
A full disk can prevent persistent error recording as well.
Recovery guidance directs the user to their own terminal to repair storage or restart the agent with `YOKE_OFF=1` for an explicit temporary bypass.
A blocking event (PreToolUse, UserPromptSubmit, Stop) that has not answered within its deadline answers itself: it denies in `enforce` mode or when the mode is still unknown, prints nothing in `observe` mode, records the overrun in `~/.yoke/error.log`, and exits, so only one decision is ever printed.
The deadline is 25 seconds for UserPromptSubmit, whose default Claude Code hook timeout is 30 seconds, and at least 120 seconds for PreToolUse and Stop, whose default hook timeout is 600 seconds in Claude Code and Codex; an agent that times out a hook lets the action through.
The PreToolUse and Stop deadline grows to the sum of the event's trusted check gate timeouts plus 10 seconds, up to 540 seconds, so a gate within its own timeout is never cut; `yoke trust` refuses gates that cannot fit.
`YOKE_HOOK_DEADLINE_MS` can only shorten it.
The watchdog records its answer as a decision without the payload; an enforce-mode Stop it denies counts toward `stop.max_blocks`, so the plan still stalls and the agent can stop.
Before exiting it kills the process groups of gates and checks still running.

### 4.2 Wire format

Claude Code and Codex share the blocking shapes below, but the writer must account for their different rewrite contracts.
Yoke prints nothing when it allows without context or a rewrite.

| Situation | Output |
|-----------|--------|
| Allow | Nothing |
| PreToolUse deny | `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"..."}}` |
| Claude PreToolUse rewrite | `{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{...}}}` |
| Codex PreToolUse rewrite | `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{...}}}` |
| Stop or UserPromptSubmit block | `{"decision":"block","reason":"..."}` |
| Context for the agent | `{"hookSpecificOutput":{"hookEventName":"<Event>","additionalContext":"..."}}` |

A strict test serializes every variant and checks it against the field sets both agents accept.
Support for `additionalContext` on SessionStart and UserPromptSubmit is checked against each agent's published schema during implementation; where an agent lacks it, yoke skips context injection for that event on that agent.

Event support:

| Event | Claude Code | Codex | Yoke uses it for |
|-------|-------------|-------|------------------|
| PreToolUse | yes | yes | Gates, self-protection, rewrites |
| PostToolUse | yes | yes | Facts, observing gates |
| PostToolUseFailure | yes | no | Command-failure facts, observing gates |
| UserPromptSubmit | yes | yes | Gates, resuming paused plans, plan context |
| Stop | yes | yes | Stop guard; on Codex also the debounced digest |
| SessionStart | yes | yes | Memory and plan context, retention pruning, daily update check |
| PreCompact | yes | yes | Observing gates |
| SessionEnd | yes | yes | Digest; Codex Stop also provides the debounced fallback |
| Notification | yes | no | Observing gates |

Agent contracts were checked on 2026-10-13 against the [Claude hooks reference](https://code.claude.com/docs/en/hooks) and [Codex hooks reference](https://learn.chatgpt.com/docs/hooks).
Codex local command hooks require local orchestration; hosted and managed execution can have different coverage.

### 4.3 Config

Global `~/.yoke/config.toml` is merged with the nearest project `yoke.toml`; the project wins.
`yoke config` prints every effective value and the file it came from.

Top-level settings:

| Key | Default | Meaning |
|-----|---------|---------|
| `mode` | `"observe"` | `observe` or `enforce` |
| `log.retention_days` | `90` | Decisions, facts and the check runs of done or cancelled plans older than this are pruned at most hourly at session start, in bounded batches; memories, trusted snapshots and unfinished plans' runs are kept |
| `log.payload_cap` | `4096` | Bytes of redacted payload kept per decision |
| `stop.max_blocks` | `5` | Consecutive stop blocks without progress before a plan stalls |
| `plans.dir` | `".yoke/plans"` | Where plan files live, relative to the project root |
| `memory.digest` | `false` | Run the session-end digest |
| `memory.digest_agent` | `"claude"` | `claude` or `codex` for the digest |
| `memory.inject_tokens` | `1500` | Token budget for session-start memory |
| `review.reviewer` | `"auto"` | `auto` picks the other agent from the one building; or `claude`, `codex` |

Gate keys (existing keys keep their meaning):

| Key | Default | Meaning |
|-----|---------|---------|
| `name`, `on` | required | As in 0.1.0 |
| `match_tool` | none | Regex fully matched against the tool name, so `Bash` and `Edit\|Write` both work |
| `match_contains` | none | Substring of the payload text |
| `match_pattern` | none | Regex searched in the payload text |
| `action` | `"check"` | `check` runs `run` and fails on non-zero exit; `deny` fails whenever it matches; `rewrite` edits the tool input |
| `run` | none | Required for `check`, unused otherwise |
| `timeout` | `30` | Seconds; a timeout fails the gate |
| `message`, `shell` | as in 0.1.0 | |
| `rewrite_field` | `"command"` | Field of `tool_input` to rewrite |
| `rewrite_pattern`, `rewrite_to` | none | Regex and replacement for `rewrite` |
| `require_command` | none | Rewrite prerequisite: executable basename must be available on PATH |

Gate commands also receive `YOKE_SESSION`, `YOKE_CWD` and `YOKE_PROJECT`.

### 4.4 Trust

Yoke never runs config straight from disk.
It runs the last snapshot a human trusted.

- `yoke trust` (in a project) validates `yoke.toml` and `.yoke/knowledge.md`, shows a diff against the current snapshot, asks for confirmation, then stores their content and sha256.
- `yoke trust --global` does the same for `~/.yoke/config.toml`.
- A file that fails to parse cannot be trusted, so a broken config never reaches a hook.
- When a file on disk differs from its snapshot, yoke keeps running the snapshot and says so in session-start context and in `yoke config`.
- A project that has never been trusted contributes no gates and no knowledge.

### 4.5 Self-protection

A built-in guard runs before user gates on PreToolUse and cannot be disabled by config.
It denies, in `enforce` mode:

- Tool calls that run `yoke trust`, `yoke console`, `yoke jobs retry`, `yoke notifications ack`, `yoke plan approve`, `yoke plan resume`, `yoke plan cancel`, `yoke plan reassign`, `yoke plan accept`, `yoke plan resolve` or `yoke plan acknowledge`.
- Write or edit tool calls whose target path is inside `~/.yoke/`.
- Shell commands that reference `state.db` or open it with `sqlite3`.

Human-only commands also refuse to run when they detect they are inside an agent tool process.
The detection signal for each agent is verified against that agent's behaviour during implementation, not assumed.

#### Threat model and limits

Both layers target accidental and casual self-unblocking, in line with principle 7; they are not a security boundary against a determined agent.
Layer 1, the guard above, matches the literal tool input of PreToolUse events, and only in `enforce` mode.
Layer 2, `require_human`, refuses when a known agent marker is set (`CLAUDECODE=1`, `CLAUDE_CODE_CHILD_SESSION=1`, a nonempty `CODEX_THREAD_ID` or `CODEX_SESSION_ID`) or when stdin or stdout is not a terminal, and most human-only commands then require a typed confirmation phrase.
`yoke console` is itself human-only, because the console can approve, resume and acknowledge.
Its session secret travels only in an `HttpOnly`, `SameSite=Strict` cookie whose `Path` is a random per-run prefix (`/c/<32 hex>/`) under which the console serves everything; cookies are not isolated by port (RFC 6265 section 8.5), so the path is what keeps the secret away from other servers on `127.0.0.1`.
The printed link and the browser opener's arguments carry a one-time launch code that expires after five minutes.

Together these stop an agent that runs a human-only command directly, starts the console, edits files in `~/.yoke` with its write tools, or opens `state.db` by name.
They cannot stop an agent that has a shell running as the same operating-system user and sets out to bypass them.
Such an agent can rewrite a command so the pattern does not match (quoting, variables, `xargs`, an interpreter's subprocess call), clear the agent markers and allocate a pseudo-terminal, open the database with its own SQLite library, replace the `yoke` binary or the hook configuration, or read whatever the user can read, such as terminal multiplexer scrollback holding an unused launch link or the browser's cookie store.
It can also race the human for an unused launch code while the browser opener runs; the human then sees an expired link, and the console reports on stderr that the link was used.
The working-tree fingerprints have a size limit in the same class.
Tracked or untracked files larger than 64 MiB (`STREAMED_HASH_LIMIT` in `plan/files.rs`) are compared by size and modification time in nanoseconds, while smaller files are content-hashed.
A deliberate same-size edit to such a file followed by an exact restore of its modification time is therefore not detected by the done-plan anchor or by the check's before/after evidence guard.
An audit actor of `human` therefore means "passed the human-terminal check", not proof that a person acted.

Containing a determined agent needs operating-system isolation, which Yoke recommends but does not provide:

- Run coding agents as a separate OS user, or in a container or virtual machine, that cannot read or write the human's `~/.yoke`, browser profile, terminal multiplexer sockets, the `yoke` binary, or the agent hook settings.
- Prefer the agent's own sandbox modes that restrict writes outside the workspace and limit network access.
- Run human-only commands and the console from a terminal the agent cannot observe or attach to.

### 4.6 Decision log

Every hook event writes one `decisions` row, allows included, so the console can show coverage and not only denials.
Payloads are redacted before storage:

- Values of keys or flags named like `token`, `secret`, `password`, `passwd`, `api_key`, `apikey`, `authorization`.
- `Bearer <value>`.
- Known token prefixes: `ghp_`, `gho_`, `ghs_`, `github_pat_`, `sk-`, `xoxb-`, `xoxp-`, `AKIA` followed by 16 characters.
- PEM blocks.

After redaction the payload is cut to `log.payload_cap` bytes on a character boundary.
`yoke log` shows recent rows with filters for project, session, outcome and tool.
`yoke trial report` summarizes the same rows read-only as release-trial evidence per agent and mode; [trials](../trials.md) defines a working day.

### 4.7 Install and uninstall

`yoke install --agent claude,codex [--statusline] [--dry-run]`:

- Backs up each file it will change to `~/.yoke/backups/<timestamp>/`.
- Adds hook entries for the events in section 4.2, each running `yoke run-hook --agent <agent> <Event>`.
- Records exact owned entries in an installation receipt and removes only those entries during uninstall.
  Existing matching hooks are not adopted; unrelated hook entries remain untouched.
  Commands use a shell-quoted resolved executable path so an installation does not accidentally invoke a different binary on PATH.
- Enables Codex's hooks feature flag if it is off; the exact key is read from Codex's documentation at implementation time.
- With `--statusline`, sets Claude Code's status line to `yoke statusline`, wrapping any existing command with `--wrap "<existing>"`.
- Writes Claude settings under `CLAUDE_CONFIG_DIR` and Codex `hooks.json` and `config.toml` under `CODEX_HOME` when those are set (they must be absolute), otherwise under `~/.claude` and `~/.codex`.
- Refuses a first install while Claude user settings set `disableAllHooks` or file-based managed settings (`managed-settings.json` and `managed-settings.d/`) set `disableAllHooks` or `allowManagedHooksOnly`; a reinstall reports these as warnings and never edits them.
- `--dry-run` prints the diff and writes nothing.
- Installing twice is a no-op.

`yoke uninstall` removes exactly the entries install added and restores a wrapped status line.
After install then uninstall, each settings file is semantically identical to its backup.

`yoke health [--json]` reads, and never writes, the state database, receipts and agent settings.
It reports each agent's receipt, owned hook entries, hook executable, Claude hook-disabling settings in managed, local, project and user files, the Codex hooks feature flag, the last recorded decision per agent, trusted policy freshness, mode and unfinished plans.
It exits 1 when installed hooks cannot run or no agent is installed; observe mode, untrusted policy and missing plans are guidance.
Codex `/hooks` approval and Claude MDM or server-managed policy are not readable locally and are reported as unknown.

### 4.8 Update

- CI builds `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` for every tag, with a `.sha256` per asset and a `THIRD_PARTY_LICENSES` file.
- `yoke update` uses `curl` to read the latest GitHub release, downloads the asset for its target next to the current binary, verifies the sha256 with the `sha2` crate, then replaces the binary with one atomic rename. On any failure the old binary stays in place.
- `yoke update --check` only records the latest version in `kv`. Session start spawns it detached at most once every 24 hours.

### 4.9 CLI surface

`init`, `gates`, `run-hook`, `install`, `uninstall`, `trust`, `config`, `log`, `update`.
Later sub-projects add `plan`, `review`, `remember`, `recall`, `share`, `forget`, `memory digest`, `memory export`, `memory import`, `console`, `statusline`, `preset`.
`notifications` lists actionable notifications, and `notifications ack <id|all>` acknowledges them from a human terminal only.
`extensions` inventories Claude Code and Codex extension definitions read-only and exits 1 on shadowed, stale or dangling definitions; locations and precedence are cited in `docs/decisions/0003-extension-inventory.md`.

## 5. Sub-project 2: Workflow

### 5.1 Plan files

A plan is Markdown with a TOML header between `+++` lines, stored in `.yoke/plans/<slug>.md`.
The header holds what the plan promises; the body holds the plan itself.
Status is never in the file.

```markdown
+++
title = "Rate-limit the login endpoint"
review = true

[[check]]
name = "tests"
run = "cargo test -p api"

[[check]]
name = "ui-lint"
run = "npx @axe-core/cli http://localhost:3000/login"
blocking = false
+++

## Goal
...
```

`yoke plan new <slug>` writes a template; `yoke plan list` and `yoke plan status [<slug>]` read state from the database.

The header may also declare `[[requirement]]` entries (`id`, `text`, and either `checks = [<blocking check names or "review">]` or `human = true`) and `[[task]]` entries (`id`, `title`, `requirements`, `depends_on`, `owner = "agent" | "human"`).
Ids are unique and use lowercase letters, digits and hyphens; dangling references and dependency cycles fail parsing, and the check name `acceptance` is reserved when either section is present.
Because they live in the header, approval freezes them with the checks.
Task completion and human requirement evidence are runtime data in `plan_completions`, never in the file.
A task is completed with `yoke plan task done` only after its dependencies; a human-owned task needs the human path.
A human records requirement evidence with `yoke plan accept`.

### 5.2 States

Main path: `draft -> approved -> building -> checking -> done`.
A failed or void check run sends `checking` back to `building`.
Side states `waiting` and `stalled` leave `building` and return to it.
`cancelled` is terminal like `done`; a cancelled plan cannot be approved, resumed or reassigned.

| Transition | Who | How |
|------------|-----|-----|
| draft to approved | Human | `yoke plan approve <slug>` in a terminal, or the console button. Freezes the header as `approved_header`. Refused while a blocking validation finding is open. |
| approved to building | Agent | `yoke plan start <slug> --session <session-id>`; binds an existing recorded session |
| building to waiting | Agent | `yoke plan ask "<question>" --slug <slug>`; records the question |
| waiting or stalled to building | Human | `yoke plan resume <slug>` in a human terminal, or the authenticated console action |
| checking to building (interrupted check) | Human | `yoke plan resume <slug>`; refused while a live process owns the check. Records an `interrupted` check run. |
| any unfinished state to cancelled | Human | `yoke plan cancel <slug> [--reason '<text>']`; records the reason and releases the session binding |
| building, waiting or stalled to the same state | Human | `yoke plan reassign <slug> --session <session-id>`; records the move and leaves only the new session bound |
| building to checking | Agent | `yoke plan check <slug> --agent <claude-or-codex>` |
| checking to done | Yoke | Every blocking check passed and the fingerprint did not change during the run |
| checking to building | Yoke | Any blocking check failed, or files changed during the run |
| building to stalled | Yoke | The stop guard reached its cap |

Every transition writes a `transitions` row with its actor.
UserPromptSubmit alone does not authenticate human intent and cannot resume a waiting or stalled plan.
If the header on disk differs from `approved_header`, `yoke plan check` refuses to run and the plan needs re-approval: an approved plan's checks cannot be weakened by editing the file.

### 5.3 Checks and evidence

`yoke plan check` runs the approved checks itself, in order, each with the gate timeout rules.
For each it stores exit code, the last 4 KB of output, duration, blocking flag and the working-tree fingerprint.

The fingerprint is the sha256 of: the `HEAD` commit id, the output of `git diff HEAD --binary`, and the path and content of every untracked file that git does not ignore.
It is computed before and after the run.
If they differ, the run is void and the plan returns to `building`.

`blocking = false` checks run and are shown but never decide the outcome.
`review = true` in the header adds a blocking `review` check (section 5.5).
Every approvable header requires at least one blocking check or required review.
When the header declares requirements or tasks, each run also records an `acceptance` result: `done` additionally requires every requirement satisfied by checks that passed in that run or by human evidence, and every task done.
Checks can declare relative PNG, JPEG, or WebP paths in `artifacts`; each must be a regular file inside the project of at most 10 MiB.
Generated screenshots belong in Git-ignored output directories so creating them does not invalidate the working-tree fingerprint.
Artifact declarations never exclude source files from the fingerprint.
Each artifact's SHA-256 is stored in `evidence.content_hash` when its check captures it.
`yoke plan status` re-hashes the files and reports each as `verified`, `tampered`, `missing` or `unrecorded` for runs recorded before hashes existed.
A running `yoke plan check` holds an exclusive advisory lock on the plan file until it finishes; the OS drops the lock if the process dies, which is how `plan resume` tells an orphaned `checking` plan from a live one.

### 5.4 Stop guard

On Stop, when the session's active plan is `building` or `checking`:

1. Compute the fingerprint. If it differs from `last_fingerprint`, or a transition or check run happened since the last block, reset `blocks` to 0.
2. If `blocks` is below `stop.max_blocks`, block with a reason naming the plan, its state and the next command, and increment `blocks`.
3. Otherwise allow the stop and move the plan to `stalled`.

When the active plan is `done`, Stop compares the current fingerprint with `done_fingerprint`.
An unchanged tree allows the stop without running checks again.
A changed tree blocks; in `enforce` mode the plan returns to `building` in the same transaction with an audited `yoke` transition and fact, and `yoke plan check` completes it again.
Observe mode records the would-be denial without changing the plan.
Standalone review evidence never changes `done_fingerprint`.

Plans in any other state never block a stop.
The count lives in `stop_guard`, so it does not depend on any flag in the agent's payload.
A session's active plan is enforced only in the plan's own project, while `sessions.project` records the project of the session's latest event; binding a plan requires that latest event to be in the plan's project, and a session owns at most one unfinished plan.

Plan context (state and next command) is injected at SessionStart and UserPromptSubmit through `additionalContext`.

### 5.5 Review

`yoke review [<slug>]` runs a reviewer agent headless on tracked changes from `git diff <base_ref>` and untracked, nonignored source files.
`base_ref` is the commit recorded when the plan started, so work committed during the plan stays in the review.
Plans started before `base_ref` was recorded fall back to `git diff HEAD`.
Review input must cover newly created files as well as edits; unsupported or oversized input fails explicitly instead of silently receiving a passing review.
`yoke plan diff <slug>` prints that exact input through the same collector, read-only and without running a reviewer.
Yoke does not create, sync or remove worktrees and performs no Git writes; see `docs/decisions/0004-worktree-and-git-review.md`.

- The reviewer is `codex exec` or `claude -p`, picked by `review.reviewer`.
- Yoke supplies its own review prompt and requires JSON output: a list of findings with `severity` (`blocking` or `advisory`), `file`, `line` and `summary`.
- The run uses the gate timeout rules with a default of 600 seconds.
- Any blocking finding fails the check. A timeout, a non-zero exit or output that does not match the schema also fails it. A review is never skipped silently.
- Findings are stored with the check run and shown in the console.

### 5.6 Plan validation and feedback

`yoke plan validate <slug> --agent <claude-or-codex>` runs the same reviewer selection on the plan text before approval, asking for requirement coverage and risky assumptions.
The reviewer must return a JSON list of findings with `severity` (`blocking` or `advice`), `anchor` (one of the plan's anchors) and `summary`; anything else fails validation and stores nothing.
Findings are stored in `plan_feedback` with the digest of the plan text they reviewed.
Approval is refused while any blocking finding is open, whatever revision it reviewed, so an edit cannot hide it; a human resolves or acknowledges each one.
`yoke plan comment` anchors feedback to `plan`, `requirement:<id>`, `task:<id>`, `check:<name>` or `heading:<slug>`; unknown anchors are rejected.
Anyone may reply, but only a human closes an item, and the closing actor, time and note are recorded.
Validation, comments and progress never write the approved header, so changing approved checks still needs fresh approval.

## 6. Sub-project 3: Memory

### 6.1 Facts

Hooks record facts with no model call:

- File writes and edits (PostToolUse on edit tools).
- Failed `Bash` or `PowerShell` commands on Claude Code, from the documented `error` text of PostToolUseFailure; interrupts are skipped. Codex documents no failure signal for tool calls, so Codex command failures are not recorded.
- Plan transitions and check results.
- Denials.

### 6.2 Remembered and digested memories

- `yoke remember "<text>"` stores a memory from the agent or the user.
- When `memory.digest` is on, a durable digest job is queued at SessionEnd on Claude Code and a detached `yoke jobs work` worker runs it. On Codex it is queued at Stop when at least 30 minutes have passed since the session's last digest and new facts exist.
- Jobs are deduplicated per session and project while queued, running or awaiting retry. A worker leases a job for 12 minutes and renews the lease before each digest batch.
- A failed attempt backs off 60 seconds, doubling up to one hour, and the job is `dead` after 3 attempts. SessionStart requeues jobs whose lease expired, prunes old finished jobs and starts a worker when work is due; there is still no daemon.
- `yoke jobs` lists queued, running, failed and dead jobs with their redacted last error; `yoke jobs retry <id>` is human-only.
- The digest sends undigested facts plus the transcript tail to one headless agent call and expects JSON memories back.
- Facts are marked `digested` only in the same transaction that stores the resulting memories, so a failed call loses nothing.

### 6.3 Recall

- At SessionStart, yoke injects pinned memories, the trusted `.yoke/knowledge.md`, and the highest-ranked memories for this project, until `memory.inject_tokens` is reached (estimated at 4 characters per token).
- Rank is FTS5 BM25 relevance when there is a query, and any query word may match; without a query, pinned items come first, then recency.
- `yoke recall "<query>"` searches with FTS5 BM25 ranking and prints ids, dates and text.
- `yoke recall --explain` prints the exact injected selection with each item's reason, rank, score, matched words and digest facts. The explanation is never injected.
- Decision record: `docs/decisions/0001-memory-retrieval.md` benchmarks lexical recall and defers semantic retrieval.
- `yoke forget <id>` deletes a memory.

### 6.4 Sharing through git

- `yoke share <id>` appends the memory to `.yoke/knowledge.md` as one entry with its date.
- That file reaches agents only through its trusted snapshot, so any edit, whether from a teammate's commit or the agent, needs `yoke trust` first.
- Yoke never writes to `CLAUDE.md`, `AGENTS.md` or other files it does not own.

### 6.5 Portable export and import

Memory must migrate between devices, servers, and environments without copying a live SQLite database.
The planned commands are `yoke memory export <file>` and `yoke memory import <file>`.
Export produces a versioned UTF-8 JSON archive containing memories, facts, timestamps, sources, pinned state, digest state, and portable record identities and relationships.
Users can export the current project or explicitly select all projects.
Project roots are recorded as metadata, never interpreted as destination write paths.
Import supports explicit project mapping so a repository at one path can move to a different path on another machine.
An ambiguous or missing mapping must be resolved before any writes.

Export reads a consistent database snapshot and writes atomically without overwriting an existing archive unless explicitly requested.
Import validates the archive version, bounded sizes, record shapes, identities, and relationships before applying one transaction.
A dry run reports project mappings, additions, duplicates, and conflicts without writing anything.
Stable export identities make repeated imports idempotent; identical records are skipped, while conflicting records fail with an actionable report rather than silently replacing local data.
Preserve local IDs through an explicit imported-identity mapping and rebuild the search index from imported records.
Import does not execute commands or transfer trust approvals, executable config, installation state, or plan approvals.
Shared knowledge content may travel as reference data, but import never writes project files or marks that content locally trusted or shared automatically.
Imported facts retain their digest status and references so migration does not silently duplicate generated memories.
The archive is local plaintext; command help explains that memory content can be sensitive and the user controls its transfer.

Acceptance requires a real CLI export from one temporary home and import into another with a different project root, followed by equivalent recall results and pinned/source/timestamp preservation.
The focused tests must also cover repeated import, conflicts, malformed or unsupported archives, atomic rollback, and a dry run that leaves destination state unchanged.

## 7. Sub-project 4: Surfaces

### 7.1 Console

`yoke console` is human-only; it binds `127.0.0.1` on a random free port, prints a one-time launch link, and opens the browser on it.
Every console URL, including pages, assets, `events`, `actions` and `launch`, lives under a random prefix `/c/<32 hex>/` derived one-way from the session secret, so it changes every run; requests outside it get 404.
The launch link works once, within five minutes, and exchanges its code for an `HttpOnly`, `SameSite=Strict` session cookie named after the port with `Path` set to that prefix; it then redirects to the console.
Browsers therefore never send the cookie to another server on `127.0.0.1`, whose URLs do not fall under the prefix.
Every other request must carry that cookie and the console's `Host`; requests without it get 403, and actions also need the same origin and a CSRF header, which blocks other sites from triggering them.
The session secret is never written to disk and never appears in a URL, process argument, terminal output or page HTML.

Pages are rendered by the Rust binary as HTML with a small amount of vanilla JS that live-updates through server-sent events.
No JS build step and no external assets.

| Page | Shows | Actions |
|------|-------|---------|
| Projects | Every recorded project: last activity, sessions, recent denials, open plans, current protection | |
| Sessions | Identity, lifecycle, last event, plan binding, mode at last event, current protection, copyable resume command; per project | |
| Plans | Header, body, transitions with actors, check runs, review findings, screenshots | Approve, Resume |
| Decisions | Filterable decision log, observe vs enforce | |
| Memory | Search, pinned, shared | Pin, Share, Forget |
| Trust and config | Effective config with origins; diff between disk and trusted snapshot | None (trust stays a terminal command) |
| Usage | Tokens and cost per day or month, session and model, with date range and UTC offset; per project | |
| Notifications | Hook errors, observe-mode would-deny summaries, waiting or stalled plans, failed checks, stale or untrusted config; unread count in the navigation; searchable recovery help | Acknowledge, Acknowledge all shown |
| Workflow settings | Activation, Yoke-owned settings with effective values and source files, snapshot trust state, choices left to the native agent | None (read-only) |

The console uses an original bounded `std::net` transport with explicit request-size, connection-count, and I/O deadline limits.
It closes ordinary connections and rejects unsupported transfer encodings rather than implementing keepalive or chunked request bodies.
This replaces the initial `tiny_http` prototype because its request-drop behavior could drain an incomplete body without an application-controlled deadline.

### 7.2 Design bar

The console must be world class: it should feel like a product people choose, not an admin page.
These are requirements, not aspirations; the 0.5 release does not ship until each one is met.

**Visual system**

- User-selected reference, 2026-10-13: a pale lavender and mint dashboard shell, white rounded panels, compact vertical navigation, indigo outline icons, and lavender, sage, sky-blue, and amber charts.
  Apply this direction to Yoke's real data and workflows, with a bold original yoke silhouette for the brand and favicon.
  Keep the application flat and usable; the reference's laptop perspective and healthcare content are presentation context, not product UI.
- Yoke's own design system, defined as tokens: a type scale, a 4 px spacing grid, colors in OKLCH, radii, shadows and motion durations.
- Light and dark themes from the same tokens, following the OS setting with a manual override.
- One deliberate visual identity, chosen and documented before the first page is built. No default component-library look, no purple gradients, no gradient heading text, no emoji as icons.
- Dense and calm: built for people who keep it open all day. Information first, chrome second.

**Interaction**

- Keyboard first: a command palette on `Cmd+K` / `Ctrl+K`, `j` and `k` to move through lists, `Enter` to open, `g` then a letter to jump between pages, `?` for the shortcut sheet. Every action works without a mouse.
- Navigation between pages feels instant: under 100 ms on localhost.
- Live updates never shift what the user is reading; new rows announce themselves and slide in only at the top of an unscrolled list.
- Destructive or consequential actions (Approve, Forget) confirm in place, without browser dialogs, and are undoable where the data model allows.

**Every state is designed**

- Each view has a designed empty state that tells the user what to do next, a skeleton loading state, an error state that shows the real error and a recovery action, and a reconnecting state when the event stream drops.

**Content rendering**

- Plans render as typeset Markdown with syntax-highlighted code blocks.
- Diffs render with syntax highlighting, in unified and split views, with collapsed unchanged regions.
- Screenshots open in a lightbox with keyboard navigation.
- Usage charts follow one chart system with consistent colors in both themes, with hover readouts and accessible data tables.
- The decision log stays smooth at 100,000 rows through pagination or virtualization.

**Accessibility**

- WCAG 2.2 AA: contrast, visible focus rings, full keyboard reach, correct landmarks and labels, `prefers-reduced-motion` respected.

**Performance budget**

- First paint under 200 ms from localhost.
- Total JavaScript under 150 KB gzipped; no external network requests of any kind.

**Proof**

- Automated browser tests capture every page in light and dark at desktop and 390 px mobile widths, and compare against approved baselines.
- An automated accessibility scan reports zero violations on every page.
- Lighthouse accessibility, best practices and performance scores are all 95 or higher.
- Every change to the console includes before and after screenshots in its pull request.

**Open decision before building 0.5:** the stack was chosen as server-rendered HTML with a little vanilla JS, before this bar was set.
The console's implementation plan must first confirm that the bar above is reachable that way, with a small prototype of the decisions page and the command palette.
If it is not, switch to a compiled single-page app embedded in the binary, with the frontend build run in CI.

### 7.3 Usage

Token counts come from the agents' transcript files on disk.
The exact transcript formats are read from real files during implementation.
Prices come from a table compiled into each release, so costs are as current as the installed version.
Accounting is advisory on every hook event: a failed import (including the 32 MiB and 1 MiB-per-line bounds) warns and leaves the gate and stop-guard decision untouched.
An import commits only while the transcript is still the file it was read from, so an older snapshot never overwrites newer or replaced usage.
Each sample keeps its transcript line time when the line has an RFC 3339 timestamp; otherwise its time is unknown.
History groups calendar days or months in one explicit UTC offset, and any aggregate with an unpriced sample shows the priced part plus the unknown samples ([decision 0009](../decisions/0009-usage-history-time-and-attribution.md)).

### 7.4 Statusline

`yoke statusline [--wrap "<cmd>"]` reads the agent's status JSON on stdin, does one indexed read, and prints one line: mode, active plan and state, a stall or recent-deny marker, context percentage when the payload has it, and an update notice from `kv`.
It never writes and never touches the network.
With `--wrap`, it runs the wrapped command with the same stdin and appends that output.

## 8. Sub-project 5: Presets

`yoke preset list` and `yoke preset add <name> [--global]` append a bundled, readable TOML block to the chosen config file, which then needs `yoke trust`.

| Preset | Contents |
|--------|----------|
| `git-safety` | `deny` gates for force push, `reset --hard`, `clean -f` and `checkout --` |
| `web-redirect` | `deny` gates for built-in web tools, with a message naming the replacement the user configures |
| `token-saver` | A `rewrite` gate that routes shell commands through a wrapper the user names, active only if the wrapper is on PATH |
| `ui` | Commented `[[check]]` templates for a UI lint command and a Playwright screenshot step, for plan headers |

Rewrite rules:

- Rewrites run only after every deny and check gate passed.
- At most one rewrite applies per event: the first matching gate in config order. It is logged as `rewrite`.
- Codex rewrites answer `allow`, which skips its ordinary permission prompt for that call; the preset documentation states this, and rewrite gates should match narrow patterns.
  Claude rewrites omit the permission decision to preserve normal permission evaluation.

Screenshots referenced by a check's output are shown on the plan's console page.

## 9. Testing

- Unit tests for each new behaviour, one test module per production module, asserting outputs and state rather than calls.
- One CLI integration test per sub-project with a temporary `HOME`, driving real commands. For Core: trust, install, a denied hook, a logged row, uninstall.
- The strict wire test from section 4.2.
- Property tests (`proptest`) for redaction: no input containing a known token shape survives redaction.
- New dev dependencies: `assert_cmd`, `tempfile`, `proptest`.

## 10. Build order

Each sub-project gets its own implementation plan and ships as a minor version.

| Version | Sub-project | Done when |
|---------|-------------|-----------|
| 0.2 | Core | Installed in observe mode next to the old harness on the maintainer's Mac for a full working day; `yoke log` shows every event; uninstall leaves settings identical to the backup |
| 0.3 | Workflow | A real feature goes from draft to done with checks run by yoke, and the stop guard stalls a looping session at the cap |
| 0.4 | Memory | A new session starts with relevant memories from the previous day within budget; export/import preserves recall across devices and remapped project paths |
| 0.5 | Surfaces | Every requirement in section 7.2 is met and proven; console approve, decisions and usage pages work in the browser; statusline refresh measured under 15 ms |
| 0.6 | Presets | `git-safety` and `token-saver` run in enforce mode for a day with no false denials |
| 1.0 | Cutover | Old harness removed; yoke in enforce mode for both agents for a week |

## 11. Out of scope

- Windows.
- Installing, starting or supervising MCP or language servers.
- Generating designs.
- Licensing, activation or telemetry code.
- Sandboxing the agent.
