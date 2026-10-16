# CLI reference

[Documentation index](README.md)

Run `yoke --help` or `yoke <command> --help` for installed-version usage.
Without a command, Yoke displays the current mode, project, and trusted gates.
`yoke --version` prints the package version.
Angle-bracket values below are placeholders to replace, not literal arguments.
A failing command prints `error:` and a `help:` line that names the subcommand that failed, and exits 1.
On a terminal that is the only copy; when stdout is not a terminal the message also goes to stderr as `yoke: <message>`.
Piping into a reader that closes early, such as `yoke log | head -1`, ends the command quietly.
Text output shows times as local `YYYY-MM-DD HH:MM` (`yoke log` adds seconds); `--json` output keeps epoch seconds.

## Policy and integration

| Command | Purpose |
| --- | --- |
| `yoke init` | Create starter `yoke.toml`, named after the current directory and with a commented `[memory]` block; preserve an existing file |
| `yoke trust [--global]` | Interactively approve exact project or global snapshots |
| `yoke config [--json]` | Inspect effective settings, origins, and snapshot freshness |
| `yoke gates` | List trusted gates |
| `yoke run-hook <event> [--agent claude\|codex]` | Read one JSON event from stdin; agent defaults to Claude |
| `yoke install [--agent claude,codex] [--dry-run] [--statusline \| --statusline-if-absent]` | Install managed agent integrations; `--statusline` adds Claude's statusline and wraps an existing command, `--statusline-if-absent` adds it only when none is set and reports any other one it left alone |
| `yoke uninstall [--agent claude,codex] [--dry-run]` | Remove owned integration entries; it does not remove the executable, the PATH line or `~/.yoke` (see [uninstalling](uninstalling.md)) |
| `yoke health [--json]` | Read-only report of the executable, state schema, mode, policy trust and staleness, each agent's hook installation, enablement and last recorded event, unfinished plans, problems and guidance; exits 1 when a check fails |
| `yoke update [--check]` | Check releases or verify and replace the current executable; when no stable release is published yet it says so and exits 0 |
| `yoke extensions [--json]` | Read-only inventory of Claude Code and Codex skills, commands, agents, plugins, MCP servers, hooks, memory and rule files, and of Yoke preset blocks, with a drift report |

Install and uninstall default to both agents.
They use Claude settings under `CLAUDE_CONFIG_DIR` and Codex files under `CODEX_HOME` when those variables are set, which must then be absolute paths.
Installing refuses while Claude user settings set `disableAllHooks`, or file-based managed settings set `disableAllHooks` or `allowManagedHooksOnly`; reinstalling reports these as warnings and never changes them.
Use `--dry-run` to inspect their planned file changes: each file is shown as changed lines, `-` removed and `+` added, with two lines of context around them, and JSON files are pretty-printed first.
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
It exits 1 when no agent has Yoke hooks installed, an installation receipt or settings file cannot be read, an installed agent's hook executable or settings entries are missing, Claude settings disable the hooks, Codex `config.toml` turns hooks off or allows managed hooks only, or the state database cannot be read.
Observe mode, untrusted or stale policy, an agent without a recorded hook event, and unfinished plans (one next step per plan state, or `yoke plan new` when there is none) are reported as guidance with exit 0.
Guidance names each untrusted policy file that exists with its own trust command; the `policy` rows carry an `exists` column, and a file that is not on disk is not work to do.
Times in the text report are local time as `YYYY-MM-DD HH:MM`; `--json` keeps epoch seconds.
For Codex, `health` counts the approval records Codex keeps for the Yoke hooks in `config.toml` and reports how many are missing; it can see that a record exists, not whether it is current.
Codex hook approval through `/hooks`, that approval's freshness, and Claude policy delivered by MDM or the claude.ai console are not otherwise visible to Yoke, and the report says so.

## Decisions and surfaces

| Command | Purpose |
| --- | --- |
| `yoke log [--project <path>] [--session <id>] [--outcome <outcome>] [--tool <name>] [--limit <n>] [--json]` | Read newest decisions |
| `yoke notifications [--unread] [--json]` | List actionable notifications for the current project and the global configuration, unread first |
| `yoke notifications ack <id\|all>` | Human-only: acknowledge one notification, or every unread one in scope |
| `yoke console [--no-open] [--port <n>]` | Human-only: serve the loopback console and open, or with `--no-open` print, a one-time launch link that signs one browser in with a session cookie; Ctrl-C stops it. It listens on 127.0.0.1 only, on a random free port unless `--port` fixes one; use `--no-open` and an SSH tunnel on a server (see [workflows](workflows.md#console-on-a-remote-server)) |
| `yoke statusline [--wrap '<command>']` | Render existing state from stdin callback JSON, with Claude's 5-hour and weekly quota when the callback carries it, optionally appending a wrapped status command |
| `yoke usage [--limit <n>] [--json]` | Read-only agent quota (percent used and left, reset times), token and estimated-cost totals per agent for today and the last 7 days, and the newest sessions with tokens, cost and duration |
| `yoke trial report [--since <date>] [--until <date>] [--agent claude\|codex] [--limit <n>] [--before <id>] [--json]` | Read-only release-trial evidence: activity, working days and outcomes per agent and mode, flagged decisions, recovery incidents and acceptance status |

The `yoke log` table shows local time with seconds, agent, event, tool, outcome, the gates that did more than pass, and a short detail: what the tool acted on, then the reason.
Rows from more than one project also get a `project` column.
`--project` takes any path inside a project and matches the project that contains it; a path that is not a directory matches as written.
`--session` and `--tool` are exact, case-sensitive matches.
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
Each row names the next command to run and the section of [troubleshooting](troubleshooting.md) that explains it.
Acknowledgement requires a human terminal, because an agent that acknowledges an alert hides it from the person it is meant for.
`ack all` acknowledges only what is already stored; it does not collect new notifications first.
Notifications follow `log.retention_days`, except that unresolved conditions stay until they clear.
Cleanup uses the current project's retention policy only for that project's and global notifications; it does not prune another project's inbox.
The list and the console cover only the project you run them from plus the global configuration; run them from another project to see its notifications.
Project identity is compared before display redaction, so paths that look identical after redaction do not share an inbox or acknowledgement authority.
Legacy notifications with an ambiguous redacted project remain stored but hidden until an exact plan or configuration source can establish ownership; decision records whose original project was already redacted cannot reconstruct that ownership.
Each run reads a bounded slice of new history in short transactions and pauses 110 ms before each one, longer than SQLite's longest busy-retry sleep, so a hook waiting for the database retries while it is free, even when runs follow each other back to back; when more remains it prints `pending: true` (with `--json`, a note on stderr), and the next run continues.
`yoke notifications` never creates or upgrades the state database: with no state it prints an empty list, and with an older schema it exits 1 and says so, because hooks that still run an older yoke refuse a newer schema.

## Plans and review

| Command | Purpose |
| --- | --- |
| `yoke plan new <slug> [--title '<title>'] [--json]` | Create an editable draft and print its path and next steps; `--json` prints the full record. The repository needs a first commit before a plan can be started or diffed |
| `yoke plan list` | List persisted project plans |
| `yoke plan status [<slug>] [--json]` | List plans, or show one plan's task and requirement progress, open feedback, check runs, and whether each captured artifact still matches its recorded hash; `--json` adds every field and the plan body |
| `yoke plan validate <slug> --agent claude\|codex [--json]` | Ask the independent reviewer to check the plan for requirement coverage and risky assumptions before approval; findings are stored as `blocking` or `advice`; `--json` prints the digest and full finding text |
| `yoke plan approve <slug>` | Human-only approval of frozen checks; refused while a blocking validation finding is open |
| `yoke plan task done <slug> <task-id> [--evidence '<text>']` | Mark an approved task done after its dependencies; tasks with `owner = "human"` need a human terminal |
| `yoke plan accept <slug> <requirement-id> --evidence '<text>'` | Human-only evidence for a requirement declared with `human = true` |
| `yoke plan comment <slug> --anchor <anchor> --text '<text>' [--json]` | Record feedback anchored to `plan`, `requirement:<id>`, `task:<id>`, `check:<name>`, or `heading:<slug>` |
| `yoke plan reply <slug> <id> --text '<text>' [--json]` | Reply to a finding or comment without changing its state; `plan status` shows the reply under the item it answers |
| `yoke plan resolve <slug> <id> [--note '<text>']` | Human-only: close a finding or comment as resolved |
| `yoke plan acknowledge <slug> <id> [--note '<text>']` | Human-only: accept the risk of a finding or comment without changing the plan |
| `yoke plan start <slug> --session <session-id>` | Bind an approved plan to an existing recorded session and record the current `HEAD` as the review base |
| `yoke plan ask '<question>' --slug <slug>` | Record a question and enter a waiting state |
| `yoke plan resume <slug>` | Human-only resume of waiting or stalled work, or recovery of a `checking` plan whose check process died |
| `yoke plan cancel <slug> [--reason '<text>']` | Human-only cancellation of an unfinished plan; records the reason and releases its session |
| `yoke plan reassign <slug> --session <session-id>` | Human-only handover of a building, waiting, or stalled plan to another recorded session |
| `yoke plan check <slug> --agent claude\|codex` | Execute checks and required review against the current tree |
| `yoke plan diff <slug> [--json]` | Read-only: show the exact change text the independent review receives, from the plan's recorded base |
| `yoke review [<slug>] --agent claude\|codex [--session <session-id>] [--json]` | Record independent review evidence on a plan that has started; prints the verdict and findings, and `--json` adds the reviewer's raw output |

`plan start`, `plan check`, `plan ask` and `plan task done` print one JSON value on stdout; `plan start` adds a note on stderr when the mode is observe.
For `plan ask`, `--session` may replace `--slug` to resolve the active plan.
Recovering an interrupted `checking` plan preserves its frozen approval even if the file was edited; the next check refuses an edited header until a human re-approves it.
`plan validate` uses the same reviewer selection as `plan check`.
For `review`, provide a slug or `--session`.
The required `--agent` identifies the building agent, allowing automatic selection of the other reviewer.
See [workflows](workflows.md) for the approval and evidence model.

## Memory and presets

| Command | Purpose |
| --- | --- |
| `yoke remember '<text>' [--pinned]` | Save a local project note with recognizable secrets redacted; with a project's first note in observe mode it says that session start injects nothing until enforce mode |
| `yoke recall [<query>] [--limit <count>] [--json]` | Search project memory; any query word may match and the best matches come first, while no query lists pinned then newest notes; default limit 100, range 1 through 1000; the text output starts with the `project:` it searched |
| `yoke recall [<query>] --explain [--json]` | Show what session start injects in enforce mode for that query, within `memory.inject_tokens`, and why each item was selected; it prints `injection: inactive` while the mode is observe, and the explanation is never injected |
| `yoke forget <id>` | Delete one memory of this project, and the project-knowledge entry `yoke share` wrote for it; exits 1 for an ID that is not in this project or when a shared entry could not be removed |
| `yoke share <id>` | Append a note of this project to project knowledge without trusting it; sharing it again changes nothing and says so |
| `yoke memory pin <id> [--unpin]` | Change pin status of a memory of this project |
| `yoke memory digest <session-id> [--transcript <path>]` | Generate memories from pending facts with the configured CLI, up to 10 batches of 100 facts per run |
| `yoke jobs [--all] [--json]` | List queued, running, failed and dead background jobs with their last error, including digests queued at session start for sessions whose end hook never ran; `--all` adds succeeded jobs. Never creates or upgrades the state database: with no state it lists nothing, and with an older schema it exits 1 |
| `yoke jobs retry <id>` | Give a dead job a fresh set of attempts and start a worker; requires a human terminal |
| `yoke memory export <file> [--all] [--overwrite]` | Export current-project or all-project memory as plaintext |
| `yoke memory import <file> --map '<source>=<destination>' [--dry-run]` | Import with explicit project-path mappings; repeat `--map` per source project, using `<source>=<source>` to restore in place; a dry run exits 1 when it finds conflicts |
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
