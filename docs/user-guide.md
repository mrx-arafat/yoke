# User guide

[Documentation index](README.md)

This guide takes you from install to daily use.
It is the short path; the [CLI reference](cli.md), [configuration reference](configuration.md), [workflows](workflows.md) and [troubleshooting](troubleshooting.md) hold every detail.
Steps marked **your terminal** can only be run by a person at an interactive terminal, never by an agent.

## 1. What Yoke is

Yoke is a local quality gate for Claude Code and Codex.
It sits in the hook path of each agent.
On every hook event it runs the checks a human approved, records what it decided, and, in enforce mode, blocks the action with a reason the agent sees.
It is one executable and one SQLite database on your machine.
It has no long-running daemon and no account.
Yoke itself makes no network requests except `yoke update` and a release check that a session start runs at most once a day.
Set trusted `update.check = false` to disable that automatic check; see [configuration](configuration.md#settings-and-defaults).
Reviews and memory digests run your own agent CLI, which uses its own connection.

An agent can say a task is done when no test proved it, and a rule written in a prompt is a request, not a guarantee.
Yoke turns such a rule into a gate that code enforces.

Yoke is not a sandbox.
Its self-protection stops an agent from casually approving its own plan, starting the console or editing Yoke state through ordinary tools.
An agent with a shell running as your operating-system user can deliberately work around it.
Trusting a configuration authorizes its shell commands to run with your permissions, and observe mode still runs them.
Use operating-system isolation if you need more protection; [Security model and limits](workflows.md#security-model-and-limits) lists what is and is not covered.

Yoke 0.1.0 is a pre-release.
No release is published yet, so the installer builds from source and `yoke update` has nothing to install.
The [README status section](../README.md#status) says what has been verified and what is still open.

## 2. The five-minute start

The commands take about five minutes.
Building from source the first time adds compile time.

1. Install with the one-line installer:

   ```sh
   curl -fsSL https://raw.githubusercontent.com/mrx-arafat/yoke/main/install.sh | bash
   ```

   On Alpine, use `| sh` instead of `| bash`, because Alpine ships without bash.
   The installer downloads a release executable and checks its SHA-256 checksum, or, while no release exists, builds from source at one exact commit.
   It connects the hooks of each agent it finds, creates the state database, adds Yoke's statusline to Claude Code when you have none, and runs `yoke health`.
   It asks before it installs a compiler, edits your shell startup file or adds the statusline, and every question defaults to yes, except wrapping a statusline you already have.
   When no trusted global policy exists and you are at a terminal, it also offers the `git-safety` preset in observe mode; trusting it is still your step.
   Rerunning it upgrades in place.
   [Getting started](getting-started.md) lists its environment variables, such as `YOKE_YES`, `YOKE_AGENTS` and `YOKE_NO_MODIFY_PATH`.

2. Open a new login shell so `yoke` is on your `PATH` if the installer added a `PATH` line, then check the install:

   ```sh
   yoke health
   ```

   The first line is `health: ok`, `health: attention` or `health: failing`, followed by a `meaning` line.
   `failing` means a check failed, the output names the problem, and the exit code is 1.
   `attention` means setup is unfinished or a step is waiting for you and nothing is broken; the exit code is 0.
   `ok` means everything Yoke checks is in place.
   Before any hooks are installed, the report says `failing` with `Yoke hooks are not installed for any agent.`
   A fresh install with hooks reports `attention` until a human trusts a policy and an agent has recorded an event.
   The rest of the report lists the executable, the state database and schema, the mode, the project, the policy files with their trust status, each agent's hooks and last event, unfinished plans, problems, and the next command to run.

3. Create a starter policy in the project you want to govern:

   ```sh
   cd /path/to/project
   yoke init
   ```

   It writes `yoke.toml` without overwriting an existing file.
   The starter is in observe mode and holds one gate, `no-force-push`, using the git-safety preset's force-push matcher for both agents' shell tools.
   It recognizes common force flags and refspecs, but cannot parse shell aliases or expansions.
   Read the file and adjust it for your agent's real payloads.

4. **Your terminal:** approve the policy.

   ```sh
   yoke trust
   ```

   It prints the file, its SHA-256 and every line, then asks you to type `trust`.
   Until you do, the file contributes no gates.
   Any later edit stays inactive until you run `yoke trust` again.
   `yoke config` and `yoke gates` show what is in force.

5. Connect the agents, if the installer has not already (add `--dry-run` first to see every file change):

   ```sh
   yoke install --agent claude,codex
   ```

   Pass `--agent claude` or `--agent codex` for one agent.
   Installing keeps your other hooks and is safe to repeat.
   Restart or reload agent sessions that were already running so they load the hooks.
   For Codex, open Codex itself, run `/hooks` and approve the Yoke hooks.
   Codex skips hooks you have not approved without any warning, including in `codex exec`, and `yoke health` reports `no approval record for N of 7 Yoke hooks` until you do.

6. Try the gate without running anything real by piping in a simulated event:

   ```sh
   printf '%s\n' '{"tool_name":"Bash","tool_input":{"command":"git push --force"}}' |
     yoke run-hook --agent claude pre_tool_use
   yoke log --limit 5
   ```

   This only simulates the event; it does not run Git.
   In observe mode Yoke prints nothing.
   With the starter policy trusted, the log shows a `would_deny` row for the `no-force-push` gate.
   Without a trusted policy, no gate runs and the row says `allow`.

## 3. Observe first, then enforce

New policy starts in observe mode, the default.
Observe mode runs the gates and records what would have been denied, but it blocks nothing, and the agent sees no difference.
No gate runs at all until a human trusts a policy.
Observe mode still runs check commands, so read them before you trust them.

You do not have to write gates by hand.
`yoke preset list` shows the bundled packs, and `yoke preset add git-safety` appends readable deny gates for force pushes, hard resets, forced cleans and `git checkout --` discards to `yoke.toml`.
Nothing applies until you review the file and run `yoke trust`.
With `--global` the preset goes to `~/.yoke/config.toml` and covers every project.

Work normally for a while, then look at what enforcement would have done:

```sh
yoke log --outcome would_deny --limit 20
```

If every row is a block you want, turn enforcement on:

1. Set the top-level `mode = "enforce"` in `yoke.toml`.
   Keep it above any `[table]` header, because keys after a header belong to that table.
2. **Your terminal:** run `yoke trust` again.

In enforce mode a failing gate on a blocking event answers in the agent's own hook format.
For a denied `PreToolUse`, Claude Code and Codex both receive:

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"Force push is blocked by Yoke. Push without --force."}}
```

A denied `UserPromptSubmit` or `Stop` answers `{"decision":"block","reason":"..."}`.
Only `pre_tool_use`, `user_prompt_submit` and `stop` can block.
A failure on any other event is recorded and the agent carries on.

Enforce mode also injects saved memory and plan context into sessions and holds `Stop` for an unfinished plan; observe mode does neither.
If Yoke itself fails, observe mode never blocks the agent, and enforce mode fails closed; see [section 11](#11-pausing-yoke-and-when-yoke-fails).

## 4. Reading a decision

`yoke log` shows the newest decisions of the project you are in.
The first line says which project (`project: "<path>"`), or `project: all` with `--all`.

```text
project: "/path/to/project"
decisions[1]{time,agent,session,event,tool,outcome,gate,detail}:
  "2026-10-17 23:00:10","claude","<session-id>","pre_tool_use","Bash","deny","no-force-push","git push --force origin main - Force push is blocked by Yoke. Push without --force."
```

| Column | Meaning |
| --- | --- |
| `time` | Local time with seconds |
| `agent` | `claude` or `codex` |
| `session` | The session id, which is the value `yoke plan start --session` takes |
| `event` | The hook event, such as `pre_tool_use` or `stop` |
| `tool` | The tool the agent used, such as `Bash` |
| `outcome` | `allow`, `deny`, `would_deny`, `rewrite` or `error` |
| `gate` | The gates that did more than pass |
| `detail` | What the tool acted on, then ` - ` and the reason |

`deny` is a block in enforce mode.
`would_deny` is the same block in observe mode, recorded but not applied.
`rewrite` means a rewrite gate changed the tool input.
`error` means a hook failed; read the reason in `detail`.

When rows come from more than one project, a `project` column follows `session`.
Filters you will use most:

```sh
yoke log --outcome would_deny
yoke log --outcome error --limit 10
yoke log --all --limit 50
yoke log --session <session-id>
yoke log --json --limit 1
```

`--limit` defaults to 100 and accepts 1 through 1000.
`--tool` and `--session` match exactly, including letter case.
`--project <path>` reads another project, and `--all` and `--project` cannot be combined.
`--json` returns the stored records with epoch seconds and the redacted payload.
An empty `--outcome deny` or `would_deny` listing tells you when no gate has run because the project's `yoke.toml` is not trusted.

## 5. Which project am I in?

Trust, gates, memory, plans and the log all belong to a project.
For a command you run, Yoke finds the project from your directory: the nearest `yoke.toml` a human trusted, else the nearest `yoke.toml`, else the git repository root, else the directory itself.
`yoke config` prints the project it resolved.

For an agent session, the project is fixed when the session starts.
It is the directory Claude Code names in `CLAUDE_PROJECT_DIR`, else the directory of the session's first event, and it stays that way for the whole session.
A directory the agent reports later can only select a `yoke.toml` that a human trusted at or below the start directory.
So changing directory, writing a new `yoke.toml` in a subdirectory, or following a symlink out of the project does not switch the policy off.
A `yoke.toml` nobody trusted is ignored and appears in a notification.

A few consequences:

- A renamed or moved directory is a new project, so run `yoke trust` again there.
- A linked git worktree is its own project for memory, plans and log, and the policy trusted for the repository applies in it until you trust the worktree's own file.
- To change the project of a running session, start a new session in the right directory.

[Where Yoke looks for your project](getting-started.md#where-yoke-looks-for-your-project) lists every layout.

## 6. Seeing Yoke inside your agents

### The Claude Code statusline

The statusline puts Yoke's state in Claude Code's prompt bar.
The installer adds it when you have no statusline.
Add it yourself with one of these:

```sh
yoke install --agent claude --statusline-if-absent
yoke install --agent claude --statusline
```

The first adds Yoke's line only when you have none, and reports any other statusline it left alone.
The second wraps your existing statusline command: Yoke's line comes first and your command's output follows.
The wrapped command must finish within 250 ms and exit 0.
Otherwise Yoke prints nothing and exits 1 with `yoke: statusline wrapper failed or exceeded 250 ms`.
`yoke uninstall` restores your earlier statusline while Yoke still owns it, and keeps any change you made since.

Segments appear in this order, and each is left out when its data is missing:

| Segment | Example | Meaning |
| --- | --- | --- |
| Mode | `yoke observe (no trusted policy)` | The mode, and a note when no trusted policy applies, which means no gate is running |
| Git | `main (3 changed)` | Branch, or `detached <sha>`, and the number of changed files |
| Decisions | `allowed 12 · would-deny 1` | This session's counts; `blocked N` replaces `would-deny` in enforce mode, and `errors N` appears when there are any |
| Plan | `plan login-rate-limit building` | The plan state, and `Stop held` while enforce mode is holding Stop |
| Quota | `5h 8% · 7d 63% (resets Mon 10:00)` | Percent used of the 5-hour and weekly windows, with the reset time of the window closest to its limit |
| Cost | cost, duration, `+40/-5` | The session's cost, time and lines changed, taken from Claude Code's callback |
| Context | `ctx 42%` and a small bar | How much of the context window is used |
| Attention | `! 2 unread` | `! recent deny`, `! stalled`, unread notifications, and the first problem `yoke health` reports |
| Update | `update <version>` | A newer release was found |

When the line is wider than the terminal, whole segments are dropped and it never wraps.
The order of dropping is the update notice, cost, context, quota, plan, decisions, git, the health problem and the attention alerts; the mode is never dropped.
Colors mark a level and never carry meaning alone, because every figure is also written out.

| Setting | Effect |
| --- | --- |
| `--style compact` | The default, one line |
| `--style detailed` | A second line with the last event and its age, the model, both quota windows with reset times, the session id and the full health problem |
| `NO_COLOR` set to any non-empty value | No color |
| `YOKE_ASCII=1` | 7-bit characters only |
| `YOKE_STATUSLINE_WIDTH=<columns>` | Use this width instead of asking the terminal |

Add `--style detailed` to either install command above to write the style into the statusline command.
The statusline is read-only and never waits for the database.
Slow facts, such as the git change count and the health problem, come from a small cache that a short background helper refreshes, so they can be missing on the first draw.
When Claude Code sends rate limits, the statusline also records them in `~/.yoke/quota-claude.json` for `yoke usage`.
See [Statusline](cli.md#statusline) for every detail.

### The live view: `yoke watch`

Codex has no statusline hook, and any agent can be watched from a second terminal pane:

```sh
yoke watch
```

It shows the current project: the mode and whether a trusted policy applies, allowed, would-deny, denied and error counts per agent today and in each agent's newest session, quota bars, unfinished plans, unread notifications, pending background jobs, whether each agent's hooks are installed, and the newest events.
A footer repeats the first problem or next step that `yoke health` gives.
It reads every two seconds, opens the database read-only, and never creates, migrates or writes anything, so it is safe to leave running.
Press `q` or Ctrl-C to quit, `f` to cycle the event list between all events, denied and would-deny only, and errors only, and `r` to read again at once.
Without a terminal, for example in a pipe, it prints one snapshot and exits 0.
`NO_COLOR` and `YOKE_ASCII=1` apply here too.

## 7. Usage and quota

```sh
yoke usage
yoke usage --limit 20
yoke usage --json
```

`yoke usage` is read-only; it never creates, migrates or writes state, so it is safe while hooks are active.
It prints four parts:

- **Quota** is a table of each agent's rate-limit windows with `used`, `left`, the reset time, the time until reset, how long ago the agent reported the figures (`observed`), and the plan when the agent reports one.
  Its outlook shows recent burn and projected exhaustion when enough fresh same-window evidence exists; stale readings are labelled explicitly.
- **Usage** totals input, output, cache-read and cache-write tokens and the estimated cost per agent, since local midnight and over the 7 x 24 hours before now.
- **Sessions** lists the newest sessions with project, models, start, duration, tokens, estimated cost and coverage.
  `--limit` sets how many, default 10, range 1 through 100.
- **Notes** say what is missing and what produces it.

Where the figures come from:

- Claude Code reports its 5-hour and weekly windows in the statusline callback.
  Yoke's statusline saves them to `~/.yoke/quota-claude.json`, so Claude quota appears only after Yoke's statusline has run in a session.
- Codex writes its rate limits into its session records.
  Yoke reads them on start, prompt, stop, end and compact hooks, and at most once per minute after tools complete.
  Fresh quota reminders and warning escalations appear [inside the interactive Codex CLI](workflows.md#quota-inside-codex).
  A Codex plan that reports only a weekly window shows only `7d`.
- Token totals come from agent transcripts, imported during those Codex refreshes or when either agent stops, ends or compacts.

Quota percentages are what each agent reported; Yoke never enforces a quota.
Exhaustion estimates assume recent burn continues and stop when readings are over five minutes old.
Set `warn_percent` under `[usage]` in trusted configuration to change the warning threshold from 85.
Costs are estimates at API prices from a built-in price table, not subscription bills.
An unpriced model shows as unknown rather than a guessed amount.
A window whose reset time has passed is marked `reset` and keeps its last figure until the agent reports the next one.
A single import reads at most the newest 32 MiB of a transcript.
A session where more was skipped shows `counted X MiB of Y MiB` in its coverage column, and its totals under-report.
[Usage and quota](workflows.md#usage-and-quota) explains each figure.

## 8. Memory

Yoke memory is a small searchable notebook for each project.
Notes belong to the project you are in, and `yoke recall` prints the project it searched.
Recognizable secrets are redacted before a note is stored: token prefixes, private key blocks, bearer headers, passwords inside URLs, and the value in `key=value` or `--key value` for names such as `password` and `api_key`.
A secret with no recognizable name or prefix is not masked, so do not save one.

```sh
yoke remember 'Run the focused tests before asking for review'
yoke recall
yoke recall tests
yoke recall --explain
```

`yoke remember` prints the saved id, local time, pin and text; add `--json` for the full stored record with epoch seconds.
`yoke recall` with no query lists pinned notes first, then the newest; with a query, any word may match and the best matches come first.
Session start injects notes only in enforce mode, and `yoke remember` says so with a project's first note.
`yoke recall --explain` shows exactly what session start would inject and why each item was chosen.
It prints `injection: inactive` while the mode is observe.
Add a query to see the rank, score and matched words of each note.

What the agent receives is fenced.
The notes arrive inside a `<yoke-memory trust="untrusted-data">` block that tells the agent they are data written by earlier sessions, tools and imports, not instructions.
Each note carries a label with why it was chosen and how it was saved, such as `pinned note #12, remember` or `approved project knowledge`.
Every line is indented, and text that imitates the fence tag is rewritten, so a note cannot end the block or pose as a label.
The injection budget is `memory.inject_tokens`, 1500 by default, and the labels come on top of it.
The fence helps the agent tell notes from instructions; it cannot force a model to ignore one, so keep notes you do not trust out of enforce mode.

Curate notes:

```sh
yoke forget <id>
yoke share <id>
yoke memory pin <id>
yoke memory pin <id> --unpin
```

- Pinned notes are injected at every session start, so **your terminal** is the only place to pin: `yoke remember --pinned` and `yoke memory pin` refuse to run from an agent.
  An agent that tries is told to save the note without `--pinned`.
  Unpinning is always allowed.
- `yoke share <id>` appends the note to `.yoke/knowledge.md` so the team can share it, and says so when it was already shared.
  The file takes effect only after **your terminal** runs `yoke trust`, so review it first, and before you commit it.
- `yoke forget <id>` deletes the note and the `.yoke/knowledge.md` entry that `yoke share` wrote, then reminds you to run `yoke trust` so trusted knowledge drops it.
- An id from another project fails with `memory <id> not found in this project`.

Move memory between machines:

```sh
yoke memory export ./yoke-memory.json --all
yoke memory import ./yoke-memory.json --map '/absolute/source/project=/absolute/destination/project' --dry-run
yoke memory import ./yoke-memory.json --map '/absolute/source/project=/absolute/destination/project'
```

The archive is plaintext, so protect it.
Without `--all`, the export covers the current project, and it refuses to replace an existing file unless you add `--overwrite`.
An import needs one `--map SOURCE=DESTINATION` per project in the archive, with absolute paths, including `SOURCE=SOURCE` to restore in place; the error lists the lines it needs.
A dry run lists the memories it would add and exits 1 when it finds conflicts.
Imported notes come in unpinned unless you add `--keep-pins`, and the real import refuses to run from an agent.
Trust and approvals do not transfer.

Automatic session digests are off by default.
To turn them on, put `digest = true` under `[memory]` in `~/.yoke/config.toml` or the project's `yoke.toml`, then run `yoke trust` in **your terminal**.
A digest needs an authenticated `claude` or `codex` CLI and sends selected session material to that agent.
`yoke jobs` lists queued, running, failed and dead background jobs with their last error, and `--all` adds succeeded ones.
`yoke jobs retry <id>` requeues a dead job from **your terminal**.

## 9. Plans

A plan is a Markdown file in Git whose TOML header declares checks, requirements and tasks.
The database, not the Markdown file, owns the workflow state, so editing the file cannot approve a plan or mark it done.
The agent's word is never evidence: Yoke runs the frozen check commands and records their output.

The repository needs a first commit before a plan can start or be diffed, and that commit needs `user.name` and `user.email` set in Git.

1. Create a draft and note the path it prints:

   ```sh
   yoke plan new login-rate-limit --title "Rate-limit the login endpoint"
   ```

   The draft holds a placeholder goal and one example check, `git diff --check`.
   Replace them with commands that prove your work:

   ```toml
   +++
   title = "Rate-limit the login endpoint"
   review = false

   [[check]]
   name = "tests"
   run = "sh tests/rate_limit.sh"

   [[requirement]]
   id = "limit"
   text = "The sixth login attempt in one minute is rejected"
   checks = ["tests"]

   [[task]]
   id = "middleware"
   title = "Add the rate-limit middleware"
   requirements = ["limit"]
   +++
   ```

   Use your own repository's commands.
   Approval needs at least one blocking check or a required review.
   [Workflows](workflows.md#plans-backed-by-evidence) lists every field.

2. Optional: ask the independent reviewer to check the plan before approval.

   ```sh
   yoke plan validate login-rate-limit --agent claude
   ```

   `--agent` names the agent that builds the plan, and the other agent reviews it.
   That reviewer's CLI must be installed and authenticated.
   Findings are stored as `blocking` or `advice`, and approval is refused while a blocking one is open.

3. **Your terminal:** approve the plan.

   ```sh
   yoke plan approve login-rate-limit
   ```

   It shows the exact contract and asks you to type `approve login-rate-limit`.
   Approval freezes the checks, requirements and tasks, so changing one later needs a fresh approval.

4. Bind the plan to a recorded session.
   Take the id from the `session` column of `yoke log --limit 5`:

   ```sh
   yoke plan start login-rate-limit --session <session-id>
   ```

   The newest row is yours only while no other session is active in the project.
   In observe mode this prints a note that a Stop will only record a `would_deny`.

5. Record progress and run the checks:

   ```sh
   yoke plan task done login-rate-limit middleware --evidence "Counts failures per account"
   yoke plan check login-rate-limit --agent claude
   ```

   The task id must be declared in the plan header.
   `yoke plan check` runs the frozen commands, records their output and artifact hashes, and marks the plan done only when every blocking check, required review, requirement and task passes on a working tree that did not change during the run.
   It exits 1 when a blocking check fails, after printing one row per check.

6. Watch it:

   ```sh
   yoke plan status login-rate-limit
   yoke plan list
   ```

   The status shows each task as `done`, `ready` or `blocked`, each requirement as satisfied by `check`, satisfied by `human` or `unsatisfied`, open feedback, and the check runs.

A plan holds `Stop` only in enforce mode.
While it is building or checking, enforce mode blocks the agent's Stop.
After `stop.max_blocks` Stop attempts without progress, 5 by default, the plan becomes `stalled` for a human instead of looping.
A done plan stays done while the working tree matches the tree its checks passed on; after any later change, the next Stop in enforce mode reopens it.

Commands only a human may run, all from **your terminal**: `yoke plan approve`, `resume`, `cancel`, `reassign`, `accept`, `resolve` and `acknowledge`, and `yoke plan task done` for a task with `owner = "human"`.

- `yoke plan resume <slug>` restarts waiting or stalled work, and recovers a plan whose check process was killed.
- `yoke plan cancel <slug> --reason '<why>'` abandons an unfinished plan.
- `yoke plan reassign <slug> --session <session-id>` hands a plan to another session.
- `yoke plan ask '<question>' --slug <slug>` is how an agent records a question and moves the plan to waiting.

A session owns at most one unfinished plan.

## 10. The console

```sh
yoke console
yoke console --no-open
```

**Your terminal:** `yoke console` serves a local web console and opens a one-time sign-in link in your browser.
`--no-open` prints the link instead, and `--port <n>` fixes the port.
It listens on `127.0.0.1` only, and it refuses to start from an agent because it can approve and resume plans.
The link works once, within five minutes, and signs that browser in with a cookie.
Ctrl-C stops the server.

It covers Projects, Sessions, Plans, Decisions, Memory, Trust & config, Usage, Notifications and Workflow settings.
Trusting a configuration stays a terminal operation.

Decisions show a short command or file path and the recorded reason before you expand a row.
Expand it for the full reason, gate results and redacted payload.
New activity refreshes the view automatically; while you read an expanded record, scroll or leave filters unapplied, use **Refresh view** when ready.
The console catches up after reconnecting, with the same protection for your reading and filters.
An empty filtered result offers **Clear filters**, which keeps the selected project.

Expanded decisions link to **View recorded policy** when the hook captured a version.
The historical page shows the loaded trusted config bytes, their hashes, and the effective settings including defaults, even after a later `yoke trust`.
Older decisions and failed captures say that no policy version was recorded; Yoke does not infer history from current files.
These archives explain configuration, not historical gate executables or plan state, and never grant trust.
Archived config text stays in the private state database and is not removed by log retention.

On a remote server, reach the console through an SSH tunnel.
Start it from the project in an interactive SSH session with `--no-open` and a fixed port:

```sh
ssh -t user@server
cd ~/my-project
yoke console --no-open --port 41234
```

Then, on the computer with the browser and within five minutes, forward the same port on both sides:

```sh
ssh -N -L 41234:127.0.0.1:41234 user@server
```

Open the printed link unchanged.
The browser must use `127.0.0.1` and the forwarded port, because the console refuses any other host name as a defense against DNS rebinding.
See [Console on a remote server](workflows.md#console-on-a-remote-server).

Notifications collect hook errors, observe-mode would-deny summaries, waiting or stalled plans, failed checks, and stale or untrusted configuration, each with the next command.
`yoke notifications` prints the same list for the current project and the global configuration.
Acknowledging one with `yoke notifications ack <id|all>` is a **your terminal** step, because an agent that acknowledges an alert hides it from you.

## 11. Pausing Yoke, and when Yoke fails

### Pausing on purpose

**Your terminal:** `yoke pause` is the one supported way to work without Yoke, for example while you repair a broken install.

```sh
yoke pause
```

It prints `pause: not active` and a private `token`.
Start the agent from your terminal with the token as `YOKE_OFF`:

```sh
YOKE_OFF=<token> claude
YOKE_OFF=<token> codex
```

That process and its children run without Yoke's hooks.
Two more forms need no restart:

```sh
yoke pause --for 30m
yoke pause --clear
```

`--for` accepts `s`, `m` or `h` units, for example `90s`, `30m` or `2h`, up to 24 hours, and skips every hook for that long, recording nothing.
`--clear` ends a pause early.
`YOKE_OFF=1`, any other value, and a `YOKE_OFF` set in an agent's settings file are ignored.
The token lives in `~/.yoke/off-token`, and Yoke's guard keeps agents from reading it, so do not paste it into a settings file or a chat.
Every skip, refused skip and pause is written to `~/.yoke/error.log`, and `yoke health` and the statusline show an active pause.
Repair the problem and restart normally to get protection back.

### When Yoke cannot read its state

Every agent action passes through a hook, so a fault in Yoke has to be safe.
These rules hold for every failure in [When Yoke itself fails](troubleshooting.md#when-yoke-itself-fails):

- Observe mode never blocks the agent because of a fault in Yoke.
- Enforce mode fails closed, and the reason names the cause and the way back.
- A `Stop` that Yoke denies only because Yoke failed is denied once.
  The agent marks the repeated Stop, and Yoke lets it through, so an agent cannot loop on a fault it cannot fix.
- Policy decisions survive storage faults.
  The audit row, memory facts and usage numbers for calls made during the fault can be lost, and the hook says so.

When Yoke cannot read its database, it cannot read the mode either.
It then uses the last known mode of the project, kept in `~/.yoke/mode-cache`.
A project last seen in observe mode is allowed through, with a warning on stderr and a line in `~/.yoke/error.log`:

> Yoke cannot read its state, so this call was allowed on the last known mode for this project, observe (recorded 2 min ago), which blocks nothing; ~/.yoke/state.db is damaged: in your own terminal move it and its -wal and -shm files aside (do not delete them), restart the agent, then run `yoke trust` again to approve your policy

A project last seen in enforce mode, a project with no record, and a record that fails its check are denied for blocking events, because letting them through would drop their gates.
Every hook that reads its policy successfully notes the project's mode there, and the record holds the newest 256 projects.
Move a damaged database aside with `mv`, as the message says, and never delete it; [Recover a damaged state database](troubleshooting.md#recover-a-damaged-state-database) lists the steps.

## 12. Where Yoke keeps things

`~/.yoke` is private to your user.
It is created with mode 0700, and `state.db`, its `-wal` and `-shm` files, `error.log` and the install backups use 0600.
They hold redacted tool inputs, memories and configuration snapshots.

| File | Holds |
| --- | --- |
| `state.db` | The decision log, memory, trust approvals, plans and usage |
| `config.toml` | The global policy, if you accepted the installer's starter or created it |
| `quota-claude.json` | The last Claude quota reading |
| `statusline-git.json`, `statusline-health.json` | Small caches the statusline reads |
| `mode-cache` | The last known mode of each project, used only when the database cannot be read |
| `off-token` | The `yoke pause` token |
| `error.log` | Errors, warnings, skips and pauses; it rotates to `error.log.1` past 1 MiB |
| `install-claude.json`, `install-codex.json` | Ownership receipts for the hooks Yoke added |
| `backups/` | Timestamped copies of your agent files taken before Yoke changed them |

Each project holds `yoke.toml`, plans under `.yoke/plans` and shared notes in `.yoke/knowledge.md`.
They are ordinary files, and whether to commit them is your call.

The decision log keeps the size and status of a tool's output, never the output itself.
For a failed shell command it keeps the first 200 characters of the error output, so a sandbox error stays diagnosable.
Decisions, facts and finished check runs older than `log.retention_days`, 90 by default, are deleted by a background job.
The database file does not shrink; SQLite reuses the freed pages.
Yoke redacts recognizable secrets, which does not make arbitrary sensitive text safe to share, so look before you attach a log to a report.

## 13. Updating and uninstalling

Rerun the installer to upgrade.
Read the [upgrade notes](upgrade-notes.md) first, because the first new executable to open the state database migrates it, and an older Yoke then refuses it.
`yoke update` checks releases, and replaces the executable after verifying its checksum and that it answers `--version`.
Until the first release is published it says `no stable release is published yet (GitHub answered 404)` and exits 0.

**Your terminal:** to uninstall, follow [uninstalling](uninstalling.md), and keep the order, because the hooks name the executable by absolute path.

1. Remove the hooks, first with a dry run:

   ```sh
   yoke uninstall --agent claude,codex --dry-run
   yoke uninstall --agent claude,codex
   ```

   It removes only what Yoke added.
   If you have not touched the files since the install, each is restored byte for byte.
   If you have, your edits stay and only Yoke's entries go.
   It never removes project policy, memory or `~/.yoke`.
2. Remove the executable, for example `rm -f ~/.local/bin/yoke`, with the path that matches how you installed.
3. Remove the `PATH` line that ends in `# added by yoke install.sh`, if you let the installer add one.
4. Optionally delete `~/.yoke`, which cannot be undone.
   Export first with `yoke memory export ~/yoke-memory.json --all`.

## 14. Troubleshooting

Start with `yoke health`, which names the problem and the next command.
Each notification names the matching section of [troubleshooting](troubleshooting.md).

| What you see | Cause | Fix |
| --- | --- | --- |
| `Yoke hooks are not installed for any agent.` | No agent is connected | Run `yoke install --agent claude,codex` in your terminal |
| `yoke gates` lists nothing, and `yoke config` says `<path>/yoke.toml is not trusted, so its settings and gates are ignored` | Nobody has trusted the file | Read it, then run `yoke trust` in your terminal |
| `yoke log --outcome would_deny` is empty and says `no gate has run` | No trusted gate, so nothing could match | Trust the policy, then repeat the action |
| An edit to `yoke.toml` has no effect, and `yoke config` shows `stale` | Yoke keeps enforcing the bytes a human trusted | Run `yoke trust` again, or restore the file |
| Failures are logged but the agent carries on | Observe mode, or the event cannot block | Set `mode = "enforce"` and run `yoke trust`; use `pre_tool_use`, `user_prompt_submit` or `stop` for gates that must block |
| `no approval record for N of 7 Yoke hooks` | Codex has not approved the hooks | Open Codex, run `/hooks` and approve them |
| `executable changed; uninstall before reinstalling` | The hooks run a different `yoke` than the one installing | Run `<new yoke> uninstall --agent <agent>`, then install again |
| `settings path differs from installation receipt` | `CLAUDE_CONFIG_DIR` or `CODEX_HOME` differs from the install | Set or unset the variable as the message says |
| `yoke: command not found` over `ssh host command`, in cron or in a script | The `PATH` line is only read by interactive shells | Use the full path, such as `~/.local/bin/yoke`; hooks always use an absolute path |
| `cannot run from an agent tool process`, or `requires a human outside an agent tool process` | A human-only command was run by an agent | Run it yourself in your terminal |
| `plan 'x' is draft, but only an approved plan can start` | The plan is not approved yet | A human runs `yoke plan approve x`, then `yoke plan start x --session <id>` |
| A plan will not finish | It is waiting, stalled, or its check was interrupted | `yoke plan status <slug>`, then `yoke plan resume <slug>` in your terminal |
| `memory 99 not found in this project`, or `yoke recall` looks empty | Memory is project scoped | Compare the `project:` line `yoke recall` prints with the project where you saved the note |
| `give one --map SOURCE=DESTINATION for each project in the archive` | An import needs a map for every source project | Add the `--map` lines the message prints |
| Saved notes do not reach the agent | Injection happens only in enforce mode | Set `mode = "enforce"` and run `yoke trust`; check with `yoke recall --explain` |
| Digests never appear | `memory.digest` is off by default | Add `digest = true` under `[memory]`, run `yoke trust`, then check `yoke jobs` |
| `Yoke cannot read its state, so this call was allowed on the last known mode` | The database is damaged or unreadable and the project was last in observe mode | Move the damaged files aside as the message says and run `yoke trust` again |
| `state database schema N is newer than supported M` | A newer Yoke wrote the database and older hooks still run | Run the newer `yoke` again, or `yoke uninstall` in your terminal |
| `trusted snapshot hash mismatch for <file>` | A trusted record in the database no longer matches its hash | Review the file and run `yoke trust` |
| `another process holds ~/.yoke/state.db locked` | A tool such as `sqlite3` has the database open | Stop the process `lsof ~/.yoke/state.db` names |
| `yoke: usage ingestion failed: <reason>` | A transcript could not be imported | Usage stays at its last good import; see `~/.yoke/error.log` |
| `release lookup failed: GitHub rate limit reached (HTTP 403)` | Unauthenticated release checks are limited per IP address | Try again later |
| The console shows "This is not the console's address" | The browser used another host name or port | Open the printed link again at exactly its address, `127.0.0.1` and the forwarded port |
| Codex says commands were blocked by the sandbox, and Yoke logged `allow` | Codex's own sandbox, not Yoke | Read the `tool_response.excerpt` in the `post_tool_use` row of `yoke log --json` |

[Troubleshooting](troubleshooting.md) has the full table of faults Yoke was tested against, with what the agent and you see for each.
When you report a problem, include the Yoke version, system, agent, failing command and a redacted message, and do not attach the state database, a memory archive or a console link.

## 15. What you gain, and what you do not

You gain visibility.
Every hook event is recorded locally with its outcome, and you can read it with `yoke log`, `yoke watch`, the statusline or the console.

You gain guardrails that code enforces.
In enforce mode, a policy you approved blocks actions you decided are out of bounds, with a reason the agent sees.
Observe mode lets you find out what a policy would block before it blocks anything.

You gain continuity.
Memory carries project context between sessions within a budget you set.
A plan carries a definition of done that a human approved and that Yoke checks by running commands, not by taking the agent's word.

You gain cost and quota awareness.
`yoke usage` shows each agent's reported quota, token totals per agent, and estimated cost per session, from local records, without a proxy or an API key.

It is cheap to run.
In one development measurement on a shared 16 GB laptop, the median hook took between 4.8 and 9.9 ms.
That is one machine and not a guarantee.

What you do not get:

- No protection from a determined agent.
  An agent running as your user with a shell can work around Yoke, so use operating-system isolation if that matters.
- No quota enforcement.
  Yoke shows what the agents report.
- No exact bill.
  Costs are estimates.
- No proof beyond what the checks cover.
  A plan proves what its commands prove.
- No finished release yet.
  Yoke 0.1.0 is a pre-release; the installer builds from source and the enforce-mode trials with real work are still open, as the [README status section](../README.md#status) says.
