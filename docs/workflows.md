# Workflow guide

[Documentation index](README.md)

## Plans backed by evidence

Plans keep the editable task description in Git while SQLite owns workflow state and evidence.
Changing Markdown alone cannot approve a plan or declare it done.
The repository needs an existing HEAD commit for working-tree fingerprints, so make an initial commit first (`git commit` needs `user.name` and `user.email` set).
Outside a repository, run `git init` first; for a bare repository use a working clone or worktree.
`yoke plan approve` displays the header one line at a time, escaping and counting non-printable or non-ASCII characters so hidden formatting cannot alter the approval preview.
Plans hold a Stop only in enforce mode: set `mode = "enforce"` in `yoke.toml` and run `yoke trust` in your own terminal once per project.
In observe mode every plan command still works, but a Stop only records a `would_deny` decision (`yoke log --outcome would_deny`), and `yoke plan start` prints a note saying so.

Create a plan:

```sh
yoke plan new format-policy --title 'Validate formatting policy'
```

Edit the file whose path the command prints; `--json` prints the full plan record instead.
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
Each check supports `name`, `run`, `timeout` (default 30 seconds), `blocking` (default true), `artifacts` (an optional list of paths), and `inputs` (optional project-relative files to freeze at approval).
For example, `inputs = ["scripts/check.sh", "Makefile"]` displays each file's SHA-256 in the approved header and checks those exact bytes again at approval and before and after running checks.
Changed or missing inputs require a human to run `yoke plan approve <slug>` again.
Inputs must be unique regular project-local files, at most 64 per check, 10 MiB per file and 64 MiB across a plan.
Declare the files your check reads explicitly; Yoke cannot infer dependencies of arbitrary shell commands.
Checks without `inputs` retain their existing behavior.
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
2. Obtain a real recorded session ID from the session column of `yoke log --limit 5`, or from the console.
   `yoke log` lists only the project you run it in, and the newest row is yours only while no other session is active in that project.
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
An explicit reviewer must also differ from the building agent; selecting the same agent is refused by `plan validate`, required review in `plan check`, and `review`.
Plans with `review = false` run their checks without selecting a reviewer.
The reviewer CLI must be installed and authenticated.
An optional `[review.models]` table chooses the model Yoke passes to each reviewer CLI; without it the CLI uses its default model, and `plan check`, `review` and `plan validate` all follow the same setting.
Review evidence records the reviewer agent and the requested model; the model that actually ran stays unknown because the reviewer's text output does not report it.
Missing authentication, malformed findings, and blocking findings fail review.
`yoke review <slug> --agent codex` can record a separate review on an existing plan.
`yoke plan diff <slug>` shows the exact text the reviewer would receive: tracked changes since the plan's recorded base commit plus untracked, nonignored files.
It runs no reviewer and changes nothing, so use it to check what review will judge, or why review input is refused as binary or above 1 MiB.
Review collection skips secret-looking paths such as `.env*`, credential and secret files, private keys and database dumps, including tracked files.
Both diff review and plan validation send a redacted JSON data envelope with a fresh random boundary, and reject a response that echoes that boundary.
Before sending, stderr lists the included and skipped diff paths and reports the destination executable and payload size.
The selected CLI sends to its configured vendor; with `auto`, Claude builds are reviewed by Codex and Codex builds by Claude.
Redaction recognizes supported credential patterns, not every possible secret, so inspect `yoke plan diff` before sending sensitive changes.
Random framing reduces prompt injection risk but cannot make an LLM verdict a security boundary.

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
A session owns at most one unfinished plan, even across projects, because the Stop guard and plan context are per session.
`yoke plan start` or `yoke plan reassign` aimed at a session that still owns an unfinished plan is refused, and the refusal names that plan, its state, and its project so the human can finish, cancel, or reassign it from that project first.
The stop guard limits repeated blocking through `stop.max_blocks`; inspect plan state rather than repeatedly retrying a blocked stop.
When that limit is reached, the stall message and stored transition include the previous state, the last blocking failure with a bounded, redacted output excerpt, and commands to inspect and resume the plan.
Wait for any live check process to exit before resuming a stalled plan.
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
A pinned note is injected at every session start, so only a person may pin: `yoke remember --pinned` and `yoke memory pin` refuse to run from an agent's tool process and tell it to save the note without `--pinned`.
Unpinning is always allowed.
Recall is scoped to the current project: the directory you run it in, or the git root when it is inside a repository.
Inside a directory without git, a subdirectory is a separate project, and `yoke recall` prints the `project:` path it searched.
`yoke forget`, `yoke share` and `yoke memory pin` take the same project-scoped IDs: an ID from another project fails with `memory <id> not found in this project` and changes nothing.
A query matches any of its words, including plural and verb forms of English words, and the best matches come first.
Without a query, recall lists pinned entries first, then the newest.
Session start injects notes only in enforce mode, so set `mode = "enforce"` in `yoke.toml` and run `yoke trust` in your own terminal.
In the default observe mode nothing is injected: `yoke recall --explain` prints `injection: inactive`, and `yoke remember` says so with the first note of a project.
Run `yoke recall --explain` to see exactly what session start injects in enforce mode and why: pinned notes, trusted knowledge, then recent notes, cut off at `memory.inject_tokens`.
The agent receives them inside a `<yoke-memory trust="untrusted-data">` block that says they are data written by earlier sessions, tools and imports and not instructions.
Each item carries a label with why it was chosen and how it was saved, such as `pinned note #12, remember` or `approved project knowledge`, and every line of a note is indented so a note cannot pose as a label or end the block (text that imitates the tag is rewritten).
The labels and the block come on top of the `memory.inject_tokens` budget.
This helps the agent tell notes from instructions; it cannot make a model ignore a note, so keep notes you do not trust out of enforce mode.
Add a query to `--explain` to see the BM25 rank, score and matched words for each selected note.
Secret redaction recognizes patterns; it does not make arbitrary sensitive notes safe to publish.
It masks private key blocks, token prefixes at the start of a word (`ghp_`, `ghu_`, `github_pat_`, `sk-`, `sk_live_`, `xox*-`, `xapp-`, `glpat-`, `npm_`, `hf_`, `AIza`, `ya29.`, `SG.` and similar), JSON web tokens, bearer and basic credentials, the password in `scheme://user:password@host`, and the value of `key=value` and `--key value` for names that contain `token`, `secret`, `password`, `passwd`, `passphrase`, `api_key`, `authorization`, `credential`, `private_key`, `cookie`, `session_id` or `signature`, or that have `pass`, `pwd`, `sig` or `auth` as a whole word (`DB_PASS`, `PWD_DB`, `sig=`).
It masks the value of `key: value` only when the name ends in one of those words (or its plural), so `Tokenizer:` and `sshSecrets.ts:` are kept.
In `key: value` the value is the rest of the line, up to a quote, a backtick or one of `,`, `;`, `}` and `]`.
Ordinary prose such as `token is required` is kept, and a secret with no recognizable name or prefix is not masked: a bare hex string, a base64 blob or a 40-character AWS secret key written without its variable name stays readable.
Notes stored by an earlier version keep their original text until you forget and save them again.

`yoke share <id>` appends the selected note to `.yoke/knowledge.md`; sharing it again changes nothing and says so.
Sharing and removing a shared entry use the same project file lock, including when separate state databases write to that project.
If another writer holds it, retry after that writer finishes; the lock releases when its process exits.
New shared entries indent every content line so note text cannot impersonate another note's marker.
Review that file before committing it or running `yoke trust` to enable trusted knowledge injection.
`yoke forget <id>` also removes the entry `yoke share` wrote for that note, then reminds you to run `yoke trust` so trusted knowledge drops it.
An entry you edited by hand is left in place and reported, and the command exits 1 so a stray copy is never silent.
Local notes and shared knowledge have separate approval and distribution steps.

Automatic session digests are disabled by default.
Enable `memory.digest` and choose `memory.digest_agent` in trusted configuration to use them: put `digest = true` under `[memory]` in `~/.yoke/config.toml` or the project's `yoke.toml`, then run `yoke trust` in your own terminal.
Until then no digest job is queued, `yoke memory digest` fails with that instruction, and facts keep being recorded for a later digest.
A manual digest uses `yoke memory digest <session-id>`, optionally with `--transcript <path>`.
Each run digests at most 10 batches of up to 100 facts, so run it again when it prints `facts_pending: true`.
Automatic digests run as durable background jobs, so one session never has two digests queued or running at once.
A failed digest retries after 60 seconds, then 120 seconds, at the next session start, and becomes `dead` after 3 attempts.
If a worker process dies, its job returns to the queue after its lease expires, at the next session start.
A session whose end-of-session hook never ran, for example a killed agent, leaves its facts pending instead of digested.
The next session start in the same project recovers them: it queues a digest for every session of that project with pending facts and no recorded activity for 30 minutes, oldest first and at most 20 sessions per start.
A session that is still working is left alone while it shows activity; after 30 minutes of silence only its facts so far are digested, without its transcript, and later facts follow at its own end.
No fact is lost or digested twice: each batch marks its facts atomically, and a session whose digest job is already queued, running, failed or dead gets no second job until that job finishes or `yoke jobs retry` requeues it.
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

To restore in place, map each project to itself (`--map '/path/to/project=/path/to/project'`); the error for a missing `--map` prints that line for every project in the archive.
Dry runs report additions and duplicates without creating the destination database, and exit 1 when they find conflicts, as the real import would.
A dry run also lists the memories the import would add, as they would be stored, each cut at 200 characters and quoted: as `preview` in the JSON report on stdout with `--json`, and as a text table on stderr.
Read it, because an archive is text that nobody in the project wrote.
An import never takes pins or pending digests from the archive: every imported note is unpinned unless a person adds `--keep-pins` to restore their own export, and pending facts are stored as already digested so an archive cannot queue digest jobs that send text to a model.
It rejects a record dated before 1970 or more than a day in the future, and, like `yoke remember --pinned`, the real import refuses to run from an agent's tool process; a dry run is allowed.
A conflict names the reason: an archive already imported under another project path is refused rather than imported a second time.
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

Use `yoke gates --verbose` to inspect the trusted commands, matchers, shell, timeout, environment allowances and expected exit code before debugging a gate.
`yoke gates --json` includes snapshot freshness; `yoke log --json` shows recorded reasons and exit codes for actual hook events.
For check diagnostics, enable `capture_output = true` on that gate and trust the change; the decision log and console Gate results then include bounded, redacted stdout/stderr tails.
Inspection never runs a gate, and observe mode still executes checks.

## Extension inventory and drift

`yoke extensions` lists every skill, command, custom agent, plugin, MCP server, hook, memory file and rule that Claude Code and Codex read from their managed, user, project and local locations, and marks the hooks and presets Yoke manages.
It then reports drift: one name defined at several scopes (with the winner each agent documents), Yoke-managed definitions that no longer match what this executable would install, and hook or MCP commands whose program is missing.
The command reads files only and never executes or starts anything.
It never prints values that could hold a secret: commands appear as the program name and an argument count, URLs as scheme and host, and environment variables and headers as names only.
Text output starts with a summary line and the drift table, then the inventory; `--json` carries every note and symlink target.
It exits 1 when a Yoke-managed hook is stale or a command is missing, so a script can stop on it; shadowed names are reported with exit 0, since an override is often deliberate.
It does not distribute or repair configuration; resolve shadowed names by renaming or removing one definition, and repair managed hooks with `yoke uninstall`, then `yoke install`; `yoke install` refuses while a managed hook is edited, with or without `--dry-run`.

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

### Console on a remote server

The console listens on 127.0.0.1 of the machine it runs on, so a browser on another computer cannot reach it directly.
Start it from the project in an interactive SSH session (`ssh -t`), with `--no-open`, and fix the port so the tunnel can be set up first:

```sh
ssh -t me@server
cd ~/my-project
yoke console --no-open --port 41234
```

On the computer with the browser, within five minutes, forward the same port number on both sides:

```sh
ssh -N -L 41234:127.0.0.1:41234 me@server
```

Then open the printed link unchanged, for example `http://127.0.0.1:41234/c/<id>/launch?code=<code>`.
Without `--port` the console picks a random free port; read it from the printed link and use it on both sides of the forward.
Over SSH the console prints this tunnel command with the actual port.
The browser must use `127.0.0.1` and the forwarded port number, because the console refuses any other `Host` as a defense against DNS rebinding.
`localhost` or a different local port shows a "This is not the console's address" page and leaves the link unused.
Without the forward the browser reports a refused connection.
`--no-open` is the clean choice on a server; without it, a missing browser opener or an SSH session with no display only prints a note and the console keeps serving.

`yoke statusline` reads callback JSON from stdin and renders recorded state without creating or migrating a database.
A missing state file, or one with no committed schema such as a zero-byte file, renders the line without recorded state instead of an error, and the mode reads `yoke (no data yet)` until a hook event has recorded one.
Callback input that is not valid JSON still prints that line, and the reason goes to stderr.
Because the agent paints stdout in its prompt bar, any failure prints nothing there; the reason goes to stderr and the exit status is 1.
When the callback JSON carries Claude rate limits, the line ends with a quota segment such as `5h 8% · 7d 63% (resets Mon 10:00)`, and the figures are recorded for `yoke usage` (see [Usage and quota](#usage-and-quota)).
`--wrap '<existing-command>'` forwards that input to an existing bounded status command and appends its output.
The 15 ms target applies to Yoke's own rendering, excluding the optional wrapper and waiting for stdin to close.
Usage amounts are API token-price estimates based on supported transcript fields and a compiled pricing table, not subscription invoices.
Unsupported models or missing billing dimensions display unknown cost, and a total that includes them shows the priced part plus the unknown samples.
An import resumes a transcript at the byte where the previous import of that session stopped, so a Stop reads only what was appended.
Concurrent imports of one session never count the same lines twice, and a replaced or truncated transcript replaces the stored usage on its next import.
The Usage page filters by inclusive date range and groups by day or month in the UTC offset shown on the page.

## Live terminal view

`yoke watch` shows what Yoke sees in the current project, in any terminal pane, so it also serves Codex, which has no statusline.
It checks the state every two seconds, reads only decisions recorded since its last reading, and repaints in place.
It opens the state database read-only and never creates, migrates or writes it, so it is safe to leave running beside your agents.
`q` or Ctrl-C quits and restores the terminal, `f` cycles the event list between all events, denied and would-deny only, and errors only, and `r` reads again at once.
Without a terminal, for example in a pipe or a script, it prints one snapshot and exits 0.

The view has these parts:

- **Header**: the project, its git branch with the number of changed files, the Yoke version and the local time, then the mode and whether a trusted policy applies.
  `no policy: gates are not running` and `policy not trusted: gates are not running` mean that no gate runs until a human trusts a policy.
- **Agents**: allowed, would-deny, denied and errors for Claude Code and Codex since local midnight, and for each agent's newest session in this project, with the time of the last event.
  A rewritten call counts as allowed.
  A count ending in `+` is a lower bound: one refresh reads at most 20,000 decisions, so a busier period is not counted in full.
  A session that started more than seven days ago is counted from seven days ago, and its row says since when.
- **Quota**: each window's percent used as a bar, colored from 50 and from 80 percent used, and its reset time, read from the same records as `yoke usage`.
- **Plans, Attention, Jobs and Hooks**: unfinished plans with their state, up to three unread notifications with the command that inspects each, pending background jobs, and whether each agent's hooks are installed, with the age of the newest event its hooks recorded in any project.
- **Events**: the newest decisions first, with local time, agent, hook event, tool, outcome, the gates that did more than pass, and what the tool acted on, relative to the project when it is inside it, with the reason.
  When the listed events ran gates, the heading also shows their median and largest gate time.
  A narrow terminal drops the hook event column first, then the gates, then the tool.
- **Footer**: the first problem or step that `yoke health` reports, refreshed every 30 seconds, and the keys.

When the state is missing, locked or unreadable, the view says why under the header and keeps running; once it can be read again, the view picks it up.
A short terminal gives the events room first: the quota block goes, then plans, attention, jobs and hooks, then the agent table.
The changed-file count runs `git status` for the project directory only, without taking the index lock, at most every five seconds and for at most two seconds; the count is left out when git fails or is slow.
Set `NO_COLOR` to any non-empty value to turn color off, and `YOKE_ASCII=1` to draw only ASCII characters.

## Usage and quota

`yoke usage` shows how much of each agent's quota is used and left, and what recent sessions cost, without opening the console.
It reads local records only and never creates, migrates or writes state, so it is safe to run while hooks are active.
Schema 15 keeps exact summaries per session and UTC day, with an indexed read for the partial boundary day, so default usage reads do not aggregate every historical sample.
Detailed dated history still reads the original samples.
`--limit <n>` sets how many recent sessions to list (default 10, range 1 through 100), and `--json` prints the whole report for scripts.

The report has these parts:

- **Quota** is an aligned table of each agent's rate-limit windows: percent used, percent left, the reset time, the time until reset, how long ago the agent reported the figures, and the plan when the agent reports one (Codex does; Claude Code does not, so it shows `-`).
  `--json` carries the same rows as structured records, including the window length in minutes, the plan and an `expired` flag.
- **Usage** totals input, output, cache-read and cache-write tokens and the estimated cost per agent, since local midnight and over the 7 x 24 hours before now.
- **Sessions** lists the newest sessions with their models, start, duration, tokens, estimated cost and coverage.
  Duration runs from the earlier of the recorded session start and the first usage sample to the session's last usage sample, so it includes any idle time.
  Coverage is `full`, or says how much of an oversize transcript was counted (see below).
- **Notes** say what is missing and what produces it, for example no state yet, a state schema older than usage times need, or an agent that has not reported its quota.

Quota percentages are what each agent reported; Yoke never enforces a quota.
The `quota_outlook` block estimates burn in percentage points/hour and when a window may reach 100% before reset, assuming that rate continues.
It requires at least one minute of same-reset observations from the most recent hour, with a last reading no more than five minutes old.
Expired or stale readings have no projection. An absent projection means insufficient evidence, not unlimited capacity.
`yoke usage --json` includes each window's bounded history, source time, staleness, burn rate, evidence span and projected exhaustion time.
The console's Usage page shows account-wide quota, before/after history and projections independently of project and date filters.
Costs are the API-price estimates described above, so an unpriced model shows as unknown rather than a guessed amount.

Where quota comes from:

- **Claude Code** reports `rate_limits.five_hour` and `rate_limits.seven_day` (percent used and reset time) in the statusline callback JSON.
  The Yoke statusline, installed with `yoke install --agent claude --statusline`, shows them and merges them into `~/.yoke/quota-claude.json`, which `yoke usage` reads.
  The statusline still never creates or migrates the state database: it writes quota atomically, only when `~/.yoke` already exists, and skips unchanged figures under a minute old. A nonblocking lock beside the file prevents concurrent sessions from overwriting newer history.
  The segment shows the most-used active window's reset time; `yoke usage` lists every reset and estimates which windows may exhaust before then.
- **Codex** records `rate_limits` in each `token_count` event of its session files.
  Yoke retains quota history while importing a session's usage on start, prompt, stop, end or compact, and at most once per minute after a tool completes. Quota is only as fresh as that import; transcript formats are not a stable provider API.
  Each window carries its own length, so a Codex plan that reports only a weekly window shows only `7d`.
  Codex also reports an account credits balance.
  Yoke does not read or store it: it is a billing figure rather than a rate-limit window, so it would put account money into the state database, logs and screenshots, and Yoke only reports usage windows.

Several sessions report the same account limits, so Yoke keeps, per window, the reading with the later reset time and, for one reset time, the higher percent.
An idle session replaying old figures therefore cannot lower them.
A window whose reset time has passed is marked `reset` and keeps its last figure until the agent reports the next one.

Yoke retains up to 64 observations per window, coalescing rapid events to roughly one point per minute while keeping the current point.
Token usage and estimated cost remain separate: subscription quota burn is measured from reported percentages, never inferred from tokens or dollars.

### Quota inside Codex

With Yoke's Codex hooks installed, the interactive CLI shows a line such as:

```text
Hook · Yoke Codex quota: 5h 90% (resets 16:00). ...
```

Yoke returns Codex's documented `systemMessage` field, which renders as a hook warning in the conversation.
A fresh reading appears once per session and at most every 15 minutes of hook activity; warnings can appear sooner.
Notifications use kind `usage_limit`, separately from `usage_partial` (incomplete transcript accounting).
Crossing the configured percentage, projected exhaustion, and reaching 100% each warn once per account window/reset, across concurrent sessions.
Acknowledging a warning does not reopen it on every hook. Expired or stale evidence resolves it.
Set `warn_percent = 75` under `[usage]` in trusted Yoke configuration to change the default threshold of 85.
Claude warnings are collected on start, prompt and stop hooks; opening Notifications also refreshes both agents' conditions.

Verified with Codex CLI 0.164.0. If Codex asks to review installed hooks, use `/hooks` to trust their exact definitions.
The interactive CLI displays these messages; `codex exec --json` did not include them in the tested version.
Codex also offers built-in footer items via `/statusline`; it does not offer Claude's arbitrary command callback for that footer.
`notify` launches an external notification command and is not used as a terminal display channel.
Without a recorded reading, the report says so and names what produces it; it never shows a guessed figure.

Yoke remembers, per session, how far it has read (a byte offset, the file's identity and a fingerprint of its first and last 4 KiB), so a Stop on a long session reads only the lines added since the previous import.
If the transcript was truncated, replaced or rewritten, that memory no longer fits and the import reads it again from the start, replacing the session's stored usage.

A single read counts at most the newest 32 MiB.
That is a first import of a transcript larger than 32 MiB, or more than 32 MiB appended between two imports.
The usage in the skipped part is not counted: Claude messages and Codex counts before the cut are skipped, and the first Codex count after the cut only sets the baseline, so skipped usage is never dated to it.
Usage counted by earlier imports stays.
Such a session is marked `counted X MiB of Y MiB` in `yoke usage` and in its `partial` field with `--json`, and a note gives how many sessions are affected, because their totals under-report.
The marker clears when a later import of that session reads the whole transcript from the start.
Each such session also raises a `usage_partial` notification with a recovery action to read its coverage.

### Bounded performance checks

After building the debug CLI, run `uv run --no-project python scripts/benchmark_hooks.py --ci` for a small regression check.
It fails if a hook's full process p99 exceeds 250 ms, verifies each hook wrote its decision, and checks seeded usage totals.
Use `--json --samples 30` for full per-event p50, p95 and p99 output with the default larger seed.
The script also runs the notification-drain job synchronously and checks its counts.
It refuses release binaries, uses an isolated home outside the repository, caps its database at 48 MiB, checks real state for test-project leaks, and removes the fixture when it finishes.
These are bounded debug measurements; the script does not prove the 700 MB release hook budget.

## Security model and limits

Yoke's self-protection keeps agents from accidentally or casually unblocking themselves; it is not a sandbox.
Two layers enforce it:

1. In `enforce` mode, a built-in guard denies agent tool calls that run a human-only command (`yoke trust`, `yoke console`, `yoke pause`, `yoke jobs retry`, `yoke notifications ack`, `yoke install`, `yoke uninstall`, `yoke update`, and `yoke plan approve`, `resume`, `cancel`, `reassign`, `accept`, `resolve`, `acknowledge`; `--dry-run` and `--check` stay allowed), write inside `~/.yoke`, write a file that registers hooks or the `yoke` executable, read the bypass token, or mention `state.db`.
   It reads shell commands word by word, so quoting, `$HOME` or `~` spellings, wrappers such as `env`, `sudo`, `nohup`, `xargs` and `timeout`, nested `bash -c`, `eval`, `$(...)`, `find -exec`, redirections, globs over `~/.yoke`, and `cd ~/.yoke` followed by a relative path are all recognized.
   The same words used as data, such as a `grep` pattern, a commit message or a `git log --grep`, are allowed.
   Text handed to an interpreter (`python3 -c`, `osascript`, `tmux send-keys`) or piped into a shell is matched as a phrase instead, so it can be denied when it only mentions a human-only command.
2. Every human-only command refuses to run when a Claude Code or Codex agent marker is set in its environment or when it is not attached to an interactive terminal, and most then ask for a typed confirmation phrase.
   `yoke install`, `yoke uninstall` and `yoke update` check the agent marker only, so the install script keeps working from your own shell.
   Agent tool calls to `yoke run-hook` are also denied by the guard, while a human can still pipe a manual event into it.
   Each recorded valid event includes an OS-derived parent PID and process name in `_yoke_parent`; this helps audit manual and forged invocations but does not authenticate the hook sender.

Which policy governs an agent's tool calls is not left to the agent either.
A session is bound to the project it started in (`CLAUDE_PROJECT_DIR` for Claude Code, else the directory of its first event), and the directory a later event reports only selects a `yoke.toml` that a human trusted at or below it.
So changing directory, writing a `yoke.toml` in a subdirectory, working outside the project, or following a symlink out of it does not turn the trusted policy off: the file nobody trusted is ignored and listed in a notification.
Check gates run in the directory of the trusted `yoke.toml`, not in the directory the agent reports, so a relative command such as `./check.sh` cannot be replaced by a look-alike in a directory the agent created.
Memory, plans and the Stop guard use the same project for the whole session.
Limits: a session that starts outside any trusted project has no project policy, a session keeps its start directory for its lifetime (resume it from the right directory, or start a new one), and a directory added to the workspace is governed by the policy of the project the session started in.
A repository's own project settings can change the environment an agent starts with, like any other setting that registers hooks, so review them as you review the repository's code.

Switching Yoke off is a human act as well.
The guard denies agent writes, edits, patches and shell writes (redirections, `sed -i`, `cp`, `mv`, `rm`, `tee`, `curl -o`) to Claude Code's `settings*.json` in `~/.claude`, in `CLAUDE_CONFIG_DIR` and in any project's `.claude`, to Codex's `hooks.json` and `config.toml` in `~/.codex` and `CODEX_HOME`, to anything in a project's `.codex`, and to the installed `yoke` executable.
An interpreter given one of those paths, or code that names one, is treated as writing it, as it is for `~/.yoke`.
A hook is skipped only when `YOKE_OFF` equals the random per-install token in `~/.yoke/off-token` (mode 0600), which only `yoke pause` prints and the guard keeps agents from reading, or while a pause set by `yoke pause --for` lasts (at most 24 hours).
`YOKE_OFF=1`, any other value, and a `YOKE_OFF` set by a settings file are ignored; every honoured or refused skip and every pause is written to `~/.yoke/error.log`.
A settings file's `env` block reaches every hook, so a hook of a release build also ignores a `HOME` that differs from the home directory in the user database when that home holds Yoke state, and says so once on stderr and in `error.log`.
Debug builds and builds made with `--features honor-home-env` honour `HOME` so tests and isolated optimized runs can use their own; the shipped binary and the installer never enable it (see [CONTRIBUTING](../CONTRIBUTING.md#testing-against-real-state)).
An unset or relative `HOME` is an error in every build.
`yoke health` and the statusline report a Claude settings file that sets `env.YOKE_OFF` or `env.HOME`, `disableAllHooks` in any settings layer Yoke can read, an active pause, and a session that is open but has recorded no hook decision for 20 minutes.
Limits: this is a reading of command words, not a sandbox.
A same-user agent can still reach these files or the token through code that builds a path in pieces, a copied toolchain, or its own tools, and it can start a process without the hooks.
`disableAllHooks` or managed policy that Yoke cannot read is invisible to it, and a session you start with the token has no hooks by design.
Only operating-system isolation closes the rest.

The console is human-only for the same reason: it can approve, resume, and acknowledge.
Its printed link carries a one-time code, and the session secret lives only in an `HttpOnly`, `SameSite=Strict` cookie, so it never appears in a URL, browser history, process arguments, or terminal output.
Browsers send a cookie to every port on the same host, so the console serves every page under a random path that changes each run and limits its cookie to that path.
Other local servers you visit in the same browser, such as an agent's dev server, therefore never receive the cookie, and the console does not need a browser profile of its own.
A same-user agent can still read your browser profile's files, including that cookie; this is an accepted risk.

The background digest, which runs outside the agent's sandbox, reads a transcript only from the job's project or an agent's transcript directory, as a resolved `.jsonl` regular file, and only its last 32 KiB.
A path anywhere else in a job, such as one a forged hook event named, is ignored and logged.

Release executables are checked against a SHA-256 file from the same release, which catches a damaged download but not a replaced release.
Each release also carries signed build provenance that you can verify yourself with `gh attestation verify`; `install.sh` and `yoke update` do not verify it for you.
[Verifying releases](verifying-releases.md) has the commands and the limits.
`install.sh` builds from source at one exact revision: a release tag, a commit you name, or the one commit `main` pointed to when it ran.
A gate, an approved plan check or `yoke update` still runs code with your full privileges, outside your agent's sandbox.
Gate children use a restricted environment, disabled core dumps and a wall-clock process-group timeout described in [configuration](configuration.md#gate-environment).
Plan checks freeze explicitly declared input hashes, not every dependency or runtime access; files can still change between validation and execution.
Only approve check commands and code you trust to run with your privileges.

What Yoke writes to disk is private to your user and bounded.
`~/.yoke` is created with mode 0700, and `state.db`, its `-wal` and `-shm` files, `error.log` and the install backups with 0600 (backup folders 0700), because they hold redacted tool inputs, memories and configuration snapshots.
An installation made by an older build is tightened the next time Yoke opens its database; if a file cannot be changed, for example because another user owns it, Yoke goes on and does not fail a hook.
The decision log keeps the size and status flags of a tool's output (`tool_response`), never the output; a shell command's error output keeps its first 200 characters so that a sandbox error stays diagnosable, and a failed tool's `error` text is cut at 256 characters.
Check gates can independently opt into bounded, redacted stdout/stderr capture with `capture_output = true`; this is off by default and follows decision-log retention.
Notes you save and facts Yoke records are redacted with the patterns described under Project memory.
Rows stored by an earlier build keep what they held until retention removes them or you forget them.

These layers stop an agent from approving its own plan, starting the console, or editing Yoke state through its ordinary tools.
They cannot stop an agent that has a shell running as your OS user and deliberately works around them.
Such an agent can still hide a command behind a shell alias, a variable, a copied or renamed `yoke` binary, an encoded payload, or string concatenation inside an interpreter, remove the agent markers and fake a terminal, use an unused console link it reads from a shared terminal or from process arguments before your browser does, open the database with its own tools, or replace the `yoke` binary or the hook settings.
Evidence comparisons have a size limit as well.
Tracked or untracked files up to 64 MiB are content-hashed, but larger ones are compared only by size and modification time to the nanosecond.
So a deliberate edit to such a file that keeps its size and restores its exact modification time is not detected by the done-plan check or by the guard that voids check evidence when files change during a run.
An actor recorded as `human` means the action passed the human-terminal check, not that a person was proven to act.

If you need protection against a determined agent, add operating-system isolation:

- Run agents as a separate OS user, or in a container or virtual machine, without access to your `~/.yoke`, browser profile, terminal multiplexer sockets, the `yoke` binary, or the agent hook settings.
- Prefer your agent's sandbox modes that restrict writes outside the workspace and limit network access.
- Approve plans and run the console from a terminal the agent cannot observe or attach to.
