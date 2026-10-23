# CLI reference

[Documentation index](README.md)

Run `yoke --help` or `yoke <command> --help` for installed-version usage.
Top-level help includes a getting-started sequence; `init`, `trust`, `install` and `health` help include examples and setup constraints.
Help reads no policy or state and works before installation.
Without a command, Yoke displays the current mode, project, and trusted gates.
`yoke --version` prints the package version.
Angle-bracket values below are placeholders to replace, not literal arguments.
An execution failure prints `error:` and a `help:` line that names the subcommand that failed, and exits 1.
Invalid arguments exit 2 and print usage on stderr; help and version requests exit 0.
Health exits 1 when it reports a failing check; a policy denial uses the agent's hook response rather than the ordinary CLI error format.
On a terminal that is the only copy; when stdout is not a terminal the message also goes to stderr as `yoke: <message>`.
Piping into a reader that closes early, such as `yoke log | head -1`, ends the command quietly.
Text output shows times as local `YYYY-MM-DD HH:MM` (`yoke log` adds seconds); `--json` output keeps epoch seconds.
Plan commands print a compact text view by default, and `--json` prints the full record.

## Policy and integration

| Command | Purpose |
| --- | --- |
| `yoke init` | Create starter `yoke.toml` at the project root (the repository root when run in a subdirectory), named after that directory and with a commented `[memory]` block; preserve an existing file |
| `yoke trust [--global]` | Interactively approve exact project or global snapshots |
| `yoke config [--json]` | Inspect the resolved project, effective settings, origins, and snapshot freshness |
| `yoke gates` | List trusted gates |
| `yoke run-hook <event> [--agent claude\|codex]` | Read one JSON event from stdin; agent defaults to Claude |
| `yoke install [--agent claude,codex] [--dry-run] [--statusline \| --statusline-if-absent] [--style compact\|detailed]` | Install managed agent integrations; `--statusline` adds Claude's statusline and wraps an existing command, `--statusline-if-absent` adds it only when none is set and reports any other one it left alone, and `--style` picks its look (see [Statusline](#statusline)) |
| `yoke uninstall [--agent claude,codex] [--dry-run]` | Remove owned integration entries; it does not remove the executable, the PATH line or `~/.yoke` (see [uninstalling](uninstalling.md)) |
| `yoke project move <old> <new> [--dry-run] [--merge] [--json]` | Human-only: move stored project records in one transaction after moving the directory; preview prints every affected table and count; existing target records require `--merge`, and conflicting plan slugs refuse the entire move |
| `yoke health [--json]` | Read-only report of the executable, state schema, mode, policy trust and staleness, each agent's hook installation, enablement and last recorded event, unfinished plans, the project's memory facts that no digest has processed yet, problems and guidance; exits 1 when a check fails |
| `yoke update [--check]` | Check releases or verify, test (`--version`) and replace the current executable; when no stable release is published yet it says so and exits 0 |
| `yoke pause [--for DURATION \| --clear]` | Human-only. Without options, print the private token that lets you start an agent with `YOKE_OFF=<token>` and Yoke's hooks skipped; `--for 30m` (or `2h`, at most 24h) skips every hook for that long, and `--clear` ends it; each use is written to `~/.yoke/error.log` |
| `yoke extensions [--json]` | Read-only inventory of Claude Code and Codex skills, commands, agents, plugins, MCP servers, hooks, memory and rule files, and of Yoke preset blocks, with a drift report |

Without `--dry-run`, `install` and `uninstall` refuse to run from an agent's tool process, as does `update` without `--check`; run them in your own terminal.
Install and uninstall default to both agents.
They use Claude settings under `CLAUDE_CONFIG_DIR` and Codex files under `CODEX_HOME` when those variables are set, which must then be absolute paths.
Installing refuses while Claude user settings set `disableAllHooks`, or file-based managed settings set `disableAllHooks` or `allowManagedHooksOnly`; reinstalling reports these as warnings and never changes them.
Use `--dry-run` to inspect only changed Yoke hooks and feature flags, with `-` removed and `+` added.
Other settings are omitted and statusline command contents are hidden.
Installation resolves the executable's canonical path and requires a regular executable file.
Install and health warn about group- or other-writable executable locations and paths inside a `target` build directory; use a stable private executable and reinstall the hooks to remove those warnings.
The self-protection guard denies `run-hook` inside an agent tool call; the manual stdin examples remain available from your own terminal.
Recorded hook payloads include `_yoke_parent.pid` and `_yoke_parent.name` from the operating system, overriding any fields submitted with those names; this is audit context, not authentication.
The parent record also includes its OS start identity, so process ID reuse does not bind a new process to an old session.
When an agent runs memory commands, Yoke matches that recorded parent to the tool's current OS ancestry and uses the session anchor instead of the tool's directory.
Missing or ambiguous provenance refuses the command with a request to start a new session in the project or use your own terminal.
Agents cannot export `--all`, and their import previews must map every destination to their anchored project; real imports and pinning still require a person.
Each injected item is capped at a quarter of `memory.inject_tokens` in characters, pins use at most half of the character budget, and the remaining space reserves room for approved knowledge and unpinned notes.
`yoke recall` still shows complete saved notes.
For `project move`, stop agents first, move the directory yourself, then pass the exact absolute old project path and the existing new directory.
Only project trust snapshots whose destination bytes equal the approved bytes are eligible to move; if any snapshot differs or is unavailable, the source project's trust is dropped as a batch and the result says `untrusted`.
An explicit merge replaces the destination's prior project trust with the verified source batch, and all replaced or dropped trust rows are counted.
Other records, including memory, sessions, plans, usage and notifications, move together; nested start directories and plan file paths follow the new root.
Global trust and files on disk are not moved.
Restart sessions in the new directory, and review and trust the new policy yourself when needed; plan checks containing old absolute paths may need reapproval.
`update` depends on a published release; the development implementation does not establish release availability.
`extensions` prints a one-line summary of counts, then drift, then the inventory: `shadowed` (same name at several scopes, with the documented winner), `stale` (a Yoke-managed hook or preset that differs from what this executable would install now) and `dangling` (a hook or MCP command whose program does not exist).
Each inventory row gives the definition's agent, kind, name, scope, Yoke ownership, origin path and detail (for a hook or MCP server, the program or URL host).
Symlinks that point outside the scanned roots, such as a skill manager's store, are counted in the summary; `--json` lists each target.
Notes about files that were not read, such as invalid JSON, are listed in text.
It executes nothing and never prints values: hook and MCP commands appear as the program name and an argument count, MCP URLs as scheme and host only, and MCP environment variables and headers as names only.
It exits 1 when it finds a `stale` or `dangling` definition.
A `shadowed` name is reported but exits 0, because an override is often deliberate; a script that wants to stop on it reads `drift[].kind` from `--json`.
See [decision 0003](decisions/0003-extension-inventory.md) for the scanned locations and their sources.
`health` never creates, migrates or writes state, settings or receipts.
The report starts with `health: ok`, `attention` or `failing` and a `meaning` line that defines it.
`attention` means setup is unfinished or a step is waiting for you and nothing is broken; it is the normal state of a fresh install and of observe mode.
`failing` means a check failed, and the exit code is 1.
The `memory` line shows how many facts of the current project no digest has turned into memories yet and whether `memory.digest` is on; with digest off and facts waiting, it says how a human turns it on.
It is information, so it does not change the status.
It exits 1 when no agent has Yoke hooks installed, an installation receipt or settings file cannot be read, an installed agent's hook executable or settings entries are missing, Claude settings disable the hooks, Codex `config.toml` turns hooks off or allows managed hooks only, or the state database cannot be read.
Observe mode, untrusted or stale policy, an agent without a recorded hook event, and unfinished plans (one next step per plan state, or `yoke plan new` when there is none) are reported as guidance with exit 0.
Guidance names each untrusted policy file that exists with its own trust command; the `policy` rows carry an `exists` column, and a file that is not on disk is not work to do.
For stale policy, guidance uses `yoke trust --global` for global configuration and `yoke trust` for project configuration and knowledge; a missing trusted file must be restored before new bytes can be approved.
Observe-mode guidance names the file that supplies the effective mode and its trust command; when mode is the default, it suggests an existing project policy, then global policy.
With no recorded event for an installed agent, guidance says to start or restart `claude` or `codex` in the project, perform a tool action, then run `yoke log --limit 10` and `yoke health`.
Times in the text report are local time as `YYYY-MM-DD HH:MM`; `--json` keeps epoch seconds.
For Codex, `health` counts the approval records Codex keeps for the Yoke hooks in `config.toml` and reports how many are missing; it can see that a record exists, not whether it is current.
A WAL database whose `-wal` and `-shm` files were deleted by another SQLite client is reported as a problem that names `yoke log --limit 1` as the way back; the read-only open creates no file (see [troubleshooting](troubleshooting.md#storage-failures-block-work)).
Codex hook approval through `/hooks`, that approval's freshness, and Claude policy delivered by MDM or the claude.ai console are not otherwise visible to Yoke, and the report says so.

## Decisions and surfaces

| Command | Purpose |
| --- | --- |
| `yoke log [--all \| --project <path>] [--session <id>] [--outcome <outcome>] [--tool <name>] [--limit <n>] [--json]` | Read the newest decisions of the current project; `--all` reads every project |
| `yoke notifications [--unread] [--json]` | List actionable notifications for the current project and the global configuration, unread first |
| `yoke notifications ack <id\|all>` | Human-only: acknowledge one notification, or every unread one in scope |
| `yoke console [--no-open] [--port <n>]` | Human-only: serve the loopback console and open, or with `--no-open` print, a one-time launch link that signs one browser in with a session cookie; Ctrl-C stops it. It listens on 127.0.0.1 only, on a random free port unless `--port` fixes one; use `--no-open` and an SSH tunnel on a server (see [workflows](workflows.md#console-on-a-remote-server)) |
| `yoke statusline [--wrap '<command>'] [--style compact\|detailed]` | Render one line (two in the detailed style) from stdin callback JSON and recorded state: mode and trust, git branch, this session's allowed and blocked actions, plan, quota, cost, context and attention, optionally appending a wrapped status command; see [Statusline](#statusline) |
| `yoke usage [--limit <n>] [--json]` | Read-only agent quota (percent used and left, reset times, freshness, recent burn and projected exhaustion), token and estimated-cost totals per agent for today and the last 7 days, and the newest sessions with tokens, cost and duration |
| `yoke watch` | Read-only live view of the current project in a terminal: decisions per agent today and in its newest session, quota, the newest events with a filter, plans, unread notifications, pending jobs, hook state and the next step `yoke health` gives; `q` or Ctrl-C quits. Without a terminal it prints one snapshot and exits 0 (see [workflows](workflows.md#live-terminal-view)) |
| `yoke trial report [--since <date>] [--until <date>] [--agent claude\|codex] [--limit <n>] [--before <id>] [--json]` | Read-only release-trial evidence: activity, working days and outcomes per agent and mode, flagged decisions, recovery incidents and acceptance status |

`yoke log` lists the decisions of the project that contains the working directory, the same project every other command uses, and its first line says which (`project: "<path>"`, or `project: all`).
`--all` lists every project instead, and `--project` takes any path inside another project and matches the project that contains it; a path that is not a directory matches as written.
`--all` and `--project` cannot be combined.
`--session` searches every project unless `--project` is also given, because a session id names one session.
The table shows local time with seconds, agent, session id, event, tool, outcome, the gates that did more than pass, and a short detail: what the tool acted on, then the reason.
The session id is the value `yoke plan start --session` takes.
Rows from more than one project also get a `project` column.
`--tool` is an exact, case-sensitive match, and so is `--session`.
An empty `--outcome deny` or `--outcome would_deny` listing says when the project has no trusted gate, so no gate has run, and names an untrusted `yoke.toml` that is the reason.
To save every decision before removing Yoke, use `yoke log --all --limit 1000 --json`.
`--outcome` is one of `allow`, `deny`, `would_deny`, `rewrite` or `error`; any other value is an error that lists them.
`--limit` defaults to 100 and accepts 1 through 1000.
`--json` returns the stored records with epoch seconds.
Each `payload` is the redacted payload cut at `log.payload_cap`, and `payload_truncated` is `true` when the cut left it as invalid JSON.

```sh
yoke log --outcome would_deny --limit 10 --json
```

`usage` never creates, migrates or writes state.
`--limit` sets how many recent sessions to list (default 10, range 1 through 100).
Quota percentages are what each agent last reported, with the time it reported them; costs are API-price estimates, not subscription bills.
[Usage and quota](workflows.md#usage-and-quota) explains where each figure comes from and what it means when one is missing.

`trial report` never creates, migrates or writes state.
Its dates are `YYYY-MM-DD` (local midnight) or RFC 3339 with an offset; `--since` is inclusive and `--until` exclusive.
When both are supplied, `--until` must be later than `--since`.
`--limit` pages flagged decisions (default 100, range 1 through 1000), and `--before` continues from the printed `next_before`.
[Trials](trials.md) defines working days and how to record the result.

Notifications are derived from stored state each time you list them: hook errors, a would-deny summary per project in observe mode, waiting or stalled plans, failed or interrupted checks, and stale or untrusted configuration.
Repeats of the same problem raise a count on one notification instead of adding rows.
A repeated hook error or check failure becomes unread again; the observe-mode summary keeps counting without reopening.
When the underlying condition clears, for example a resumed plan or a passing check, an unread notification is acknowledged by `yoke` instead of you.
Each row names the next command to run and links the section of [troubleshooting](troubleshooting.md) that explains it as a full URL, because an installed machine has no `docs` folder.
For an untrusted or stale configuration the next command is `yoke trust` (`yoke trust --global` for the global file), which only a person at a terminal can run, after reviewing the file with `yoke config`.
Acknowledgement requires a human terminal, because an agent that acknowledges an alert hides it from the person it is meant for.
`ack all` acknowledges only what is already stored; it does not collect new notifications first.
Notifications follow `log.retention_days`, except that unresolved conditions stay until they clear.
Cleanup uses the current project's retention policy only for that project's and global notifications; it does not prune another project's inbox.
The list and the console cover only the project you run them from plus the global configuration; run them from another project to see its notifications.
Project identity is compared before display redaction, so paths that look identical after redaction do not share an inbox or acknowledgement authority.
Legacy notifications with an ambiguous redacted project remain stored but hidden until an exact plan or configuration source can establish ownership; decision records whose original project was already redacted cannot reconstruct that ownership.
Each run reads a bounded slice of new history in short transactions and pauses 110 ms before each one, longer than SQLite's longest busy-retry sleep, so a hook waiting for the database retries while it is free, even when runs follow each other back to back (the session-start hook runs the same collection with one slice per table and no pause, so it never sleeps, and older history drains over later hooks and runs); when more remains it prints `pending: true` (with `--json`, a note on stderr), and the next run continues.
`yoke notifications` never creates or upgrades the state database: with no state it prints an empty list, and with an older schema it exits 1 and says so, because hooks that still run an older yoke refuse a newer schema.

## Statusline

`yoke statusline` renders one line for Claude Code's prompt bar from the callback JSON on stdin and Yoke's own state.
It is read-only and never waits: it creates and migrates nothing, and anything slow comes from a small cache that a detached helper refreshes.
Segments appear in this order, and each one is left out when its data is missing:

| Segment | Shows |
| --- | --- |
| Mode | `yoke observe` or `yoke enforce`, and `(no trusted policy)` when no trusted configuration applies, which means no gate is running |
| Git | The branch, or `detached <sha>`, read from `.git/HEAD` (worktrees and submodules included), and `(N changed)` from a count cached for a few seconds |
| Decisions | This session's `allowed N`, then `would-deny N` in observe mode or `blocked N` in enforce mode (a zero is shown), and `errors N` when there are any |
| Plan | `plan <slug> <state>`, and `Stop held` while enforce mode is holding Stop for a plan being built or checked |
| Quota | Percent used of the 5-hour and weekly windows, colored by level, and the reset time of the window closest to its limit |
| Cost | The session's cost, duration and lines added and removed, from the callback |
| Context | Percent of the context window used, with a small bar |
| Attention | `! recent deny`, `! stalled`, `! N unread`, and the first problem `yoke health` reports |
| Update | `update <version>` when a newer release was found |

When the line is wider than the terminal, whole segments are dropped and it never wraps.
Segments are dropped in this order: the update notice, cost, context, quota, plan, decisions, git, a health problem, then the attention alerts; the mode is never dropped.
The width comes from `YOKE_STATUSLINE_WIDTH`, then `COLUMNS`, then the controlling terminal, and is 120 columns when none answers.
A command wrapped with `--wrap` keeps the room it needs.

Colors mark a level, quota and context below 60 percent, up to 85 percent, and above: green, yellow and red.
They never carry meaning alone, because every figure is also written out.

| Setting | Effect |
| --- | --- |
| `--style compact` | The default: one line |
| `--style detailed` | Adds a second line with the last event and its age, the model, both quota windows with their reset times, the session id and the full health problem |
| `NO_COLOR` set to any non-empty value | No color |
| `YOKE_ASCII=1` | 7-bit characters only: `,` instead of the middle dot, `[###-----]` for the bar, `...` for a cut |
| `YOKE_STATUSLINE_WIDTH=<columns>` | Use this width instead of asking the terminal |

`yoke install --statusline --style detailed` writes the style into the statusline command, and the same flag with `compact` changes it back; a line Yoke owns keeps its style when the flag is omitted.
The git count runs `git status` only in the session's own directory, with a two-second limit and without taking repository locks.
A repository too slow for that shows its branch alone and is not retried for a minute.

## Plans and review

| Command | Purpose |
| --- | --- |
| `yoke plan new <slug> [--title '<title>'] [--json]` | Create an editable draft and print its path and next steps; `--json` prints the full record. The repository needs a first commit before a plan can be started or diffed |
| `yoke plan list [--json]` | List persisted project plans with their bound session, or `none`; `--json` prints full records, using `null` for an unbound session |
| `yoke plan status [<slug>] [--json]` | List plans, or show one plan's bound session, task and requirement progress, open feedback, check runs, and whether each captured artifact still matches its recorded hash; `--json` adds every field and the plan body; an unbound session is `null` in JSON |
| `yoke plan validate <slug> --agent claude\|codex [--json]` | Ask the independent reviewer to check the plan for requirement coverage and risky assumptions before approval; findings are stored as `blocking` or `advice`; `--json` prints the digest and full finding text |
| `yoke plan approve <slug> [--json]` | Human-only approval of frozen checks; refused while a blocking validation finding is open |
| `yoke plan task done <slug> <task-id> [--evidence '<text>'] [--json]` | Mark an approved task done after its dependencies; tasks with `owner = "human"` need a human terminal |
| `yoke plan accept <slug> <requirement-id> --evidence '<text>' [--json]` | Human-only evidence for a requirement declared with `human = true` |
| `yoke plan comment <slug> --anchor <anchor> --text '<text>' [--json]` | Record feedback anchored to `plan`, `requirement:<id>`, `task:<id>`, `check:<name>`, or `heading:<slug>` |
| `yoke plan reply <slug> <id> --text '<text>' [--json]` | Reply to a finding or comment without changing its state; `plan status` shows the reply under the item it answers |
| `yoke plan resolve <slug> <id> [--note '<text>'] [--json]` | Human-only: close a finding or comment as resolved |
| `yoke plan acknowledge <slug> <id> [--note '<text>'] [--json]` | Human-only: accept the risk of a finding or comment without changing the plan |
| `yoke plan start <slug> --session <session-id> [--json]` | Bind an approved plan to an existing recorded session and record the current `HEAD` as the review base |
| `yoke plan ask '<question>' --slug <slug> [--json]` | Record a question and enter a waiting state |
| `yoke plan resume <slug> [--json]` | Human-only resume of waiting or stalled work, or recovery of a `checking` plan whose check process died |
| `yoke plan cancel <slug> [--reason '<text>'] [--json]` | Human-only cancellation of an unfinished plan; records the reason and releases its session |
| `yoke plan reassign <slug> --session <session-id> [--json]` | Human-only handover of a building, waiting, or stalled plan to another recorded session |
| `yoke plan check <slug> --agent claude\|codex [--json]` | Execute checks and required review against the current tree; prints one row per check with the end of the output of any that failed, and `--json` adds fingerprints and the stored output tails |
| `yoke plan diff <slug> [--json]` | Read-only: show the exact change text the independent review receives, from the plan's recorded base |
| `yoke review [<slug>] --agent claude\|codex [--session <session-id>] [--json]` | Record independent review evidence on a plan that has started; prints the verdict and findings, and `--json` adds the reviewer's raw output |

Every plan command prints a compact text view with the next step, and `--json` prints the full stored record as one JSON value on stdout; `plan start` adds a note on stderr when the mode is observe.
The human-only commands that ask for confirmation show the plan first as text, and `--json` replaces only the result they print after the confirmation.
`plan check` still exits 1 when a blocking check fails, after printing the rows.
For `plan ask`, `--session` may replace `--slug` to resolve the active plan.
Recovering an interrupted `checking` plan preserves its frozen approval even if the file was edited; the next check refuses an edited header until a human re-approves it.
`plan validate` uses the same reviewer selection as `plan check`.
For `review`, provide a slug or `--session`.
The required `--agent` identifies the building agent, allowing automatic selection of the other reviewer.
An explicitly configured reviewer must also be the other agent. A plan with `review = false` does not select a reviewer.
See [workflows](workflows.md) for the approval and evidence model.

## Memory and presets

| Command | Purpose |
| --- | --- |
| `yoke remember '<text>' [--pinned] [--json]` | Save a local project note with recognizable secrets redacted; text output shows its id, local time, pin and saved text; `--json` preserves the full record and epoch timestamp; with a project's first note in observe mode it says that session start injects nothing until enforce mode; `--pinned` (like `yoke memory pin`) refuses to run from an agent's tool process |
| `yoke recall [<query>] [--limit <count>] [--json]` | Search project memory; any query word may match and the best matches come first, while no query lists pinned then newest notes; default limit 100, range 1 through 1000; the text output starts with the `project:` it searched and shows each note's local `time`, while `--json` keeps epoch seconds |
| `yoke recall [<query>] --explain [--json]` | Show what session start injects in enforce mode for that query, within `memory.inject_tokens`, and why each item was selected; it prints `injection: inactive` while the mode is observe, and the explanation is never injected |
| `yoke forget <id>` | Delete one memory of this project, and the project-knowledge entry `yoke share` wrote for it; exits 1 for an ID that is not in this project or when a shared entry could not be removed |
| `yoke share <id>` | Append a note of this project to project knowledge without trusting it; sharing it again changes nothing and says so |
| `yoke memory pin <id> [--unpin]` | Change pin status of a memory of this project |
| `yoke memory digest <session-id> [--transcript <path>]` | Generate memories from pending facts with the configured CLI, up to 10 batches of 100 facts per run |
| `yoke jobs [--all] [--json]` | List queued, running, failed and dead background jobs with their last error, including the hourly `retention` cleanup and digests queued at session start for sessions whose end hook never ran; `--all` adds succeeded jobs. Never creates or upgrades the state database: with no state it lists nothing, and with an older schema it exits 1 |
| `yoke jobs retry <id>` | Give a dead job a fresh set of attempts and start a worker; requires a human terminal |
| `yoke memory export <file> [--all] [--overwrite] [--json]` | Export current-project or all-project memory as plaintext; print counts as text, or the full report with `--json` |
| `yoke memory import <file> --map '<source>=<destination>' [--dry-run] [--keep-pins] [--json]` | Import with explicit project-path mappings; repeat `--map` per source project, using `<source>=<source>` to restore in place; print counts and conflicts as text, or the full report with `--json`; a dry run exits 1 when it finds conflicts and lists the memories it would add (`preview` in JSON, a text table on stderr); imported notes are unpinned unless `--keep-pins` is given, records dated outside 1970 to tomorrow are rejected, and the real import refuses to run from an agent's tool process |
| `yoke preset list` | List the bundled packs: gate packs `git-safety`, `web-redirect` and `token-saver`, and the `ui` plan-check templates |
| `yoke preset add <name> [--global]` | Append readable policy for later human trust |

`yoke jobs work` is the internal worker that hooks and `yoke jobs retry` start in the background; it is hidden from `--help` and needs no manual use.
Preset options also include `--replacement`, `--wrapper`, `--command-prefix`, and `--acknowledge-rewrite-permissions`.
Use `yoke preset add --help` and the generated TOML when configuring a pack.
Do not invent memory IDs or session IDs; obtain them from recall results, decision records, or the console.
Memory IDs are checked against the project you run the command in.

## Output and errors

Use explicit `--json` options for automation where provided.
Many mutation and workflow commands also emit JSON records, while other commands use compact text.
Ordinary command failures exit with status 1 and include a redacted diagnostic.
Hook commands follow agent-specific output contracts, so an empty stdout alone does not establish that an action was allowed; inspect the decision log and invoking agent's response.
`yoke watch` turns color off when `NO_COLOR` is set to any non-empty value or `TERM` is `dumb`, and draws only ASCII characters when `YOKE_ASCII=1`.
