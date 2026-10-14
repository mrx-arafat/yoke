# Workflow guide

[Documentation index](README.md)

## Plans backed by evidence

Plans keep the editable task description in Git while SQLite owns workflow state and evidence.
Changing Markdown alone cannot approve a plan or declare it done.
The repository needs an existing HEAD commit for working-tree fingerprints.

Create a plan:

```sh
yoke plan new format-policy --title 'Validate formatting policy'
```

Edit the file path returned by the command.
A plan uses TOML frontmatter delimited by `+++`:

```toml
+++
title = "Validate formatting policy"
review = false

[[check]]
name = "format"
run = "cargo fmt --all -- --check"
timeout = 30
blocking = true
+++
```

Write the task description after the closing delimiter.
This example check is appropriate for the Yoke Rust workspace; use the actual verification commands for your own repository.
Each check supports `name`, `run`, `timeout` (default 30 seconds), `blocking` (default true), and `artifacts` (an optional list of paths).
Approval requires at least one blocking check or required review.

### Requirements and tasks

A plan may declare acceptance conditions and tasks in the same header, so approval freezes them together with the checks:

```toml
[[requirement]]
id = "format-clean"
text = "The workspace passes the formatting policy"
checks = ["format"]

[[requirement]]
id = "docs-reviewed"
text = "A maintainer has read the updated contributor guide"
human = true

[[task]]
id = "fix-format"
title = "Format the workspace"
requirements = ["format-clean"]

[[task]]
id = "review-guide"
title = "Review the contributor guide"
requirements = ["docs-reviewed"]
depends_on = ["fix-format"]
owner = "human"
```

Ids use lowercase letters, digits, and hyphens and must be unique.
Each requirement names blocking checks (or `review` when `review = true`) or sets `human = true`, never both.
Task `requirements` and `depends_on` must name declared ids, and dependency cycles are rejected when the plan is parsed.
`owner` is `agent` (the default) or `human`.
Plans without these sections behave exactly as before.

Record task progress with `yoke plan task done <slug> <task-id> --evidence '<what changed>'` while the plan is building, waiting, or stalled.
A task cannot be marked done before every task it depends on.
A task owned by a human can only be completed from a human terminal; completing an agent task is an unverified agent assertion.
A human records evidence for a `human = true` requirement with `yoke plan accept <slug> <requirement-id> --evidence '<what was verified>'`.
Progress is stored in the database against the approved definition of each item, so evidence for an item that is later edited and re-approved no longer counts.

When a plan declares requirements or tasks, `yoke plan check` records an `acceptance` result alongside the checks.
The plan reaches `done` only when every requirement is satisfied by checks that passed in that run or by human evidence, and every task is done.
`yoke plan status <slug>` shows each task as `done`, `ready`, or `blocked`, and each requirement as satisfied by `check`, satisfied by `human`, or `unsatisfied`.

1. A human runs `yoke plan approve format-policy` in their own interactive terminal and reviews the exact contract.
2. Obtain a real recorded session ID from `yoke log --json` or the console.
3. Run `yoke plan start format-policy --session <session-id>` with that ID.
4. After making the intended changes, run `yoke plan check format-policy --agent codex` when Codex built them, or use `--agent claude` for Claude.
5. Inspect `yoke plan status format-policy` for transitions, check output, and evidence.

Yoke executes frozen commands rather than accepting the agent's assertion that tests passed.
Each declared artifact is hashed with SHA-256 when its check captures it.
`yoke plan status <slug>` compares those hashes with the files now on disk and reports each artifact as `verified`, `tampered`, `missing`, or `unrecorded` (captured before hashes were recorded).
The working-tree fingerprint must remain unchanged while checks run before completion can be recorded.
If checks modify files, fix the tree and rerun checks against the final state.
Changing the approved contract requires fresh human approval.

### Review and human questions

Set `review = true` to require independent review.
With `review.reviewer = "auto"`, Yoke selects the agent other than the building agent named by `--agent`.
The reviewer CLI must be installed and authenticated.
An optional `[review.models]` table chooses the model Yoke passes to each reviewer CLI; without it the CLI uses its default model, and `plan check`, `review` and `plan validate` all follow the same setting.
Review evidence records the reviewer agent and the requested model; the model that actually ran stays unknown because the reviewer's text output does not report it.
Missing authentication, malformed findings, and blocking findings fail review.
`yoke review <slug> --agent codex` can record a separate review on an existing plan.
`yoke plan diff <slug>` shows the exact text the reviewer would receive: tracked changes since the plan's recorded base commit plus untracked, nonignored files.
It runs no reviewer and changes nothing, so use it to check what review will judge, or why review input is refused as binary or above 1 MiB.

### Validate a plan before approval

`yoke plan validate <slug> --agent codex` sends the plan text to the same independent reviewer before work starts and asks it for requirement coverage gaps and risky assumptions.
Each finding is stored as `blocking` or `advice` together with the SHA-256 of the plan text it reviewed.
`yoke plan approve` is refused while any blocking finding is open, even if the plan was edited after validation, so editing cannot hide a finding.
A human closes a finding with `yoke plan resolve <slug> <id>` when the plan was fixed, or `yoke plan acknowledge <slug> <id>` to accept the risk.
Advice and open comments are shown during approval but do not block it.
A failed or malformed reviewer response stores nothing and the command fails.

Anyone can record feedback with `yoke plan comment <slug> --anchor <anchor> --text '<text>'`.
Anchors stay stable across edits: `plan`, `requirement:<id>`, `task:<id>`, `check:<name>`, or `heading:<slug>`, where the heading slug is the lowercase heading text with spaces replaced by hyphens.
Unknown anchors are rejected, and `yoke plan status` marks feedback whose anchor later disappears.
Agents may answer with `yoke plan reply <slug> <id> --text '<text>'`, but only a human resolves or acknowledges an item, and the closing actor and time are recorded.
None of these commands change the approved header; editing approved checks still requires fresh approval.

Use `yoke plan ask 'Which behavior should be kept?' --slug <slug>` to record a question and move into a waiting state.
A human resumes waiting or stalled work with `yoke plan resume <slug>`.
If a `yoke plan check` process is killed, the plan stays in `checking`; `yoke plan resume <slug>` returns it to `building` and records the interrupted check as invalid evidence.
Yoke refuses while a live process still owns the check, so a running check is never taken over.
Recovery also works if the plan header was edited after the interruption; the transition records the mismatch, and the next check still requires a human to approve the edited header with `yoke plan approve <slug>`.
A human abandons an unfinished plan with `yoke plan cancel <slug> --reason '<why>'`, which records the reason and releases the plan's session.
A human hands a building, waiting, or stalled plan to a replacement session with `yoke plan reassign <slug> --session <session-id>`.
Reassignment keeps the approved checks and review base, records the move as a transition, and leaves only the new session bound.
The stop guard limits repeated blocking through `stop.max_blocks`; inspect plan state rather than repeatedly retrying a blocked stop.
A done plan stays bound to its session and is accepted at Stop while the working tree matches the tree its checks passed on.
Committing exactly the bytes the checks passed on keeps a done plan done; a plan completed before Yoke recorded that content anchor still reopens after any new commit.
After any later change, the next Stop in enforce mode reopens the plan as `building` with an audited transition and blocks until `yoke plan check` passes again; observe mode only records the would-be denial.
Standalone `yoke review` evidence never refreshes a completed plan.
The plan's own project scopes this enforcement, so a session that worked in another project still finds its plan when it returns.
`yoke plan check` owns a persistent, private per-plan lock file in Git metadata, so atomically saving the plan cannot release or replace check ownership.
Check commands and their leftover children never inherit that lock; Yoke refuses to run if Git metadata is not writable or advisory locking is unavailable.
Do not remove these lock files while checks may be running; process exit releases ownership without removing the file.

## Project memory

```sh
yoke remember 'Formatting checks must not rewrite files.' --pinned
yoke recall 'formatting' --json
```

Use the returned ID with `yoke memory pin <id>`, `yoke memory pin <id> --unpin`, or `yoke forget <id>`.
Recall is scoped to the current project.
A query matches any of its words, including plural and verb forms of English words, and the best matches come first.
Without a query, recall lists pinned entries first, then the newest.
Run `yoke recall --explain` to see exactly what session start injects and why: pinned notes, trusted knowledge, then recent notes, cut off at `memory.inject_tokens`.
Add a query to `--explain` to see the BM25 rank, score and matched words for each selected note.
Secret redaction recognizes patterns; it does not make arbitrary sensitive notes safe to publish.

`yoke share <id>` appends the selected note to `.yoke/knowledge.md`.
Review that file before committing it or running `yoke trust` to enable trusted knowledge injection.
Local notes and shared knowledge have separate approval and distribution steps.

Automatic session digests are disabled by default.
Enable `memory.digest` and choose `memory.digest_agent` in trusted configuration to use them.
A manual digest uses `yoke memory digest <session-id>`, optionally with `--transcript <path>`.
Each run digests at most 10 batches of up to 100 facts, so run it again when it prints `facts_pending: true`.
Automatic digests run as durable background jobs, so one session never has two digests queued or running at once.
A failed digest retries after 60 seconds, then 120 seconds, at the next session start, and becomes `dead` after 3 attempts.
If a worker process dies, its job returns to the queue after its lease expires, at the next session start.
Inspect jobs with `yoke jobs`, and requeue a dead job with `yoke jobs retry <id>` in your own terminal.
This requires an authenticated CLI and sends selected session material to that agent.

### Transfer memory

```sh
yoke memory export ./yoke-memory.json --all
```

Protect the plaintext archive and copy it to the destination environment.
Replace both placeholders with absolute paths and repeat `--map` for every source project, including unchanged paths:

```sh
yoke memory import ./yoke-memory.json \
  --map '<absolute-source-project>=<absolute-destination-project>' --dry-run
yoke memory import ./yoke-memory.json \
  --map '<absolute-source-project>=<absolute-destination-project>'
```

Dry runs report additions and duplicates without creating the destination database.
Imports preserve identities and fact relationships, skip identical imports, and reject conflicts atomically.
Archives are versioned and bounded to 16 MiB and 20,000 records.
Trust, approvals, credentials, and installer state do not transfer.
Imported shared notes remain local until deliberately shared and trusted at the destination.

## Presets

```sh
yoke preset list
yoke preset add git-safety
```

Inspect the appended TOML, then run `yoke trust` yourself.
Other bundled packs are `web-redirect`, `token-saver`, and `ui`.
Token-saver requires an explicit executable wrapper, command prefix, and acknowledgment of Codex rewrite permissions.
The UI pack supplies commented plan-check templates; replace them with real browser commands and screenshot paths before approving a plan.

## Extension inventory and drift

`yoke extensions` lists every skill, command, custom agent, plugin, MCP server, hook, memory file and rule that Claude Code and Codex read from their managed, user, project and local locations, and marks the hooks and presets Yoke manages.
It then reports drift: one name defined at several scopes (with the winner each agent documents), Yoke-managed definitions that no longer match what this executable would install, and hook or MCP commands whose program is missing.
The command reads files only and never executes or starts anything.
It never prints values that could hold a secret: commands appear as the program name and an argument count, URLs as scheme and host, and environment variables and headers as names only.
It exits 1 when drift is found so a script can stop on it.
It does not distribute or repair configuration; resolve shadowed names by renaming or removing one definition, and repair managed hooks with `yoke install`.

## Console and statusline

`yoke console` must be started by a human in their own terminal; it serves the local console and opens a one-time launch link, and `--no-open` prints that link instead.
The link works once, within five minutes, and signs that browser in with a session cookie; restart the console for a new link or another browser.
Ctrl-C stops the server.
The console covers Projects, Sessions, Plans, Decisions, Memory, Trust & config, Usage, Notifications, and Workflow settings.
Notifications shows actionable problems with an unread count in the navigation, a How to recover section on each item, and searchable recovery help; Acknowledge uses the same session cookie and CSRF protection as other console actions.
Notifications and its count cover the launch project and the global configuration only.
After an upgrade with a long history, older notifications appear over the first few page loads, because each load reads at most a quarter second of history.
Workflow settings is read-only: it shows observe or enforce activation, each Yoke-owned setting with its effective value and source file, the trust state of every configuration snapshot, and which choices stay with Claude Code or Codex.
Trust & config uses the same trust-state wording and shows the trusted content that still applies when its disk file has been deleted.
Projects lists every recorded project; Decisions, Sessions and Usage accept a project selection, while Plans, Memory and Trust & config stay on the launch project.
Sessions show a resume command to copy (`claude --resume <id>` or `codex resume <id>`); Yoke never runs it.
Cmd+K or Ctrl+K opens the command palette; `?` shows shortcuts.
Memory deletion offers a 30-second Undo, and plan approval displays the frozen contract before confirmation.
Configuration trust remains a terminal operation.

`yoke statusline` reads callback JSON from stdin and renders recorded state without creating or migrating a database.
A missing state file, or one with no committed schema such as a zero-byte file, renders the line without recorded state instead of an error.
`--wrap '<existing-command>'` forwards that input to an existing bounded status command and appends its output.
The 15 ms target applies to Yoke's own rendering, excluding the optional wrapper and waiting for stdin to close.
Usage amounts are API token-price estimates based on supported transcript fields and a compiled pricing table, not subscription invoices.
Unsupported models or missing billing dimensions display unknown cost, and a total that includes them shows the priced part plus the unknown samples.
Concurrent imports of one session keep the newest transcript snapshot, and a replaced or truncated transcript replaces the stored usage on its next import.
The Usage page filters by inclusive date range and groups by day or month in the UTC offset shown on the page.

## Security model and limits

Yoke's self-protection keeps agents from accidentally or casually unblocking themselves; it is not a sandbox.
Two layers enforce it:

1. In `enforce` mode, a built-in guard denies agent tool calls that literally run a human-only command (`yoke trust`, `yoke console`, `yoke jobs retry`, `yoke notifications ack`, and `yoke plan approve`, `resume`, `cancel`, `reassign`, `accept`, `resolve`, `acknowledge`), write inside `~/.yoke`, or reference `state.db`.
2. Every human-only command refuses to run when a Claude Code or Codex agent marker is set in its environment or when it is not attached to an interactive terminal, and most then ask for a typed confirmation phrase.

The console is human-only for the same reason: it can approve, resume, and acknowledge.
Its printed link carries a one-time code, and the session secret lives only in an `HttpOnly`, `SameSite=Strict` cookie, so it never appears in a URL, browser history, process arguments, or terminal output.
Browsers send a cookie to every port on the same host, so the console serves every page under a random path that changes each run and limits its cookie to that path.
Other local servers you visit in the same browser, such as an agent's dev server, therefore never receive the cookie, and the console does not need a browser profile of its own.
A same-user agent can still read your browser profile's files, including that cookie; this is an accepted risk.

These layers stop an agent from approving its own plan, starting the console, or editing Yoke state through its ordinary tools.
They cannot stop an agent that has a shell running as your OS user and deliberately works around them.
Such an agent can disguise a command so the guard pattern does not match, remove the agent markers and fake a terminal, use an unused console link it reads from a shared terminal or from process arguments before your browser does, open the database with its own tools, or replace the `yoke` binary or the hook settings.
Evidence comparisons have a size limit as well.
Tracked or untracked files up to 64 MiB are content-hashed, but larger ones are compared only by size and modification time to the nanosecond.
So a deliberate edit to such a file that keeps its size and restores its exact modification time is not detected by the done-plan check or by the guard that voids check evidence when files change during a run.
An actor recorded as `human` means the action passed the human-terminal check, not that a person was proven to act.

If you need protection against a determined agent, add operating-system isolation:

- Run agents as a separate OS user, or in a container or virtual machine, without access to your `~/.yoke`, browser profile, terminal multiplexer sockets, the `yoke` binary, or the agent hook settings.
- Prefer your agent's sandbox modes that restrict writes outside the workspace and limit network access.
- Approve plans and run the console from a terminal the agent cannot observe or attach to.
