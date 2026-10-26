# Troubleshooting

[Documentation index](README.md)

Yoke records actionable problems as notifications: hook errors, would-deny summaries in observe mode, waiting or stalled plans, failed or interrupted checks, stale or untrusted configuration, and partly counted session usage.
Read them with `yoke notifications` or on the console Notifications page, which also has a searchable copy of the recovery steps below.
Recovery links in tagged release builds point to that release's documentation.
Local builds and manual packages without a matching release tag use the documentation on `main`; no release needs to exist for those links to work.
Each notification names the section of this page that explains it.
For command examples and setup constraints, use `yoke --help` or `yoke <command> --help`; help works without state or trusted policy.
Record tables shorten long values with `…`; use `--json` for full text and identifiers, or set `COLUMNS=160` for wider previews.
Notifications put recovery commands in a separate next-steps table; full recovery URLs are in `yoke notifications --json` under `doc`.
On session start, hooks materialize at most 2,000 old decisions and queue one `notifications-drain` job for the rest.
The existing job worker drains that history in paced transactions, so catching up does not require hundreds of later session starts.
Run `yoke jobs --all` to inspect the job; failed jobs retry with backoff, and a human can run `yoke jobs retry <id>` for a dead job.

## A gate does not run

Start with these read commands from the governed project:

```sh
yoke config
yoke gates
yoke log --limit 10 --json
```

Check the resolved project, gate source, trust status, and snapshot freshness.
`yoke init` creates a file but does not trust it.
Editing or adding a preset also requires fresh trust from your own terminal.
The nearest trusted project configuration wins discovery, and a nearer file that is not trusted is ignored, for a hook as well as for a command you run; inspect origins when running from nested directories, and see [Yoke treats a directory as a different project](#yoke-treats-a-directory-as-a-different-project).

If the gate is present, compare its event and matchers with an actual recorded payload.
`match_tool` matches the whole tool name, while `match_contains` and `match_pattern` inspect searchable payload text.
The starter's `Bash` matcher does not match every Codex shell tool name.
For agent integration, inspect `yoke install --agent claude,codex --dry-run` and review newly installed Codex hooks using `/hooks`.

## Failures are logged but actions continue

Observe mode is the default.
Set top-level `mode = "enforce"` and trust the changed snapshot to enable blocking.
Post-tool and lifecycle events cannot veto an action, even in enforce mode.
Use `pre_tool_use`, `user_prompt_submit`, or `stop` when the action must be blocked.
Observe mode still runs shell checks; it suppresses blocking and rewrites, not execution.
It also does not block Stop and does not inject plan or memory context into the agent.
Review what enforce mode would have denied with `yoke log --outcome would_deny --limit 20`, then confirm the active mode and its source file with `yoke config`.
`yoke health` names the policy supplying observe mode and the trust command to use after changing it; global policy requires `yoke trust --global`.

## Configuration changed after it was trusted

Yoke keeps enforcing the exact bytes a human trusted.
An edited, deleted or unreadable file on disk has no effect until new bytes are trusted.
Compare trusted and disk content on the console Trust & config page, or look for `stale` in `yoke config`.
If the edit is intended, run `yoke trust` (or `yoke trust --global` for `~/.yoke/config.toml`) in your own terminal.
If it is not intended, restore the file.
A deleted file cannot be trusted; the trusted snapshot keeps applying meanwhile.
Health names the scope-specific trust command and tells you to restore a missing trusted file before reviewing and approving new bytes.

## Configuration is not trusted

A `yoke.toml`, `~/.yoke/config.toml` or `.yoke/knowledge.md` without a trusted snapshot contributes no settings, gates or knowledge.
`yoke init` creates a starter file but does not trust it.
Review the file, then run `yoke trust` (or `yoke trust --global`) in your own terminal.
Project knowledge is trusted together with the project `yoke.toml`, which must exist.
`yoke health` names each untrusted file that exists with the command that trusts it (`yoke trust` or `yoke trust --global`), and `yoke gates` and `yoke config` add a `help:` line saying the file is ignored.
Files that do not exist are listed with `exists` false and are not work to do: most projects have no `.yoke/knowledge.md`.
Agents cannot trust configuration.

## Hooks are disabled or missing

Yoke protects nothing when its hooks do not run.
Run `yoke health` to see which agents have Yoke hooks installed and what stops them.
When an installed agent has no recorded event, follow health's launch command (`claude` or `codex`) from the project, perform a normal tool action, then run `yoke log --limit 10` and `yoke health` again.
Health also reports group or other access to `~/.yoke` or `state.db`, with the exact `chmod 700` or `chmod 600` command to restore private permissions.
It reports these permissions without changing them.
For hook executable paths, unprotected world-write access is a problem; a sticky directory such as `/tmp` is accepted when its next path component is owned by you or root.
Group-write access produces guidance in health and a warning during install, naming the location and a `chmod g-w` command if shared write access is unnecessary; it does not make health fail.
An unreadable installation receipt is reported as a problem rather than a missing installation; repair the reported receipt before retrying installation.
An empty or zero-byte state database is reported as empty or incomplete without hiding installed agents; initialize it from your own terminal with `yoke config`, or let the next agent hook initialize it.
Yoke reads Claude Code settings from `CLAUDE_CONFIG_DIR` and Codex settings from `CODEX_HOME` when they are set to absolute paths, and from `~/.claude` and `~/.codex` otherwise.
Codex's top-level `allow_managed_hooks_only = true` also prevents Yoke's user hooks from running; `yoke health` reports the same blocker as installation.

For Claude Code, the highest-precedence file that sets `disableAllHooks` decides: the project `.claude/settings.local.json`, then the project `.claude/settings.json`, then the user `settings.json`.
If that file sets it to `true`, Yoke's hooks do not run; remove the key or set it to `false` there.
`disableAllHooks` or `allowManagedHooksOnly` set to `true` in file-based managed settings also stops them: `managed-settings.json` or `managed-settings.d/*.json` in `/Library/Application Support/ClaudeCode` on macOS and `/etc/claude-code` elsewhere.
`allowManagedHooksOnly` has effect only in managed settings ([settings reference](https://code.claude.com/docs/en/settings-reference.md), [managed settings](https://code.claude.com/docs/en/managed-settings.md)), so only an administrator can change it.
Managed policy delivered by MDM or the claude.ai console is not visible to Yoke.
`yoke health` and the statusline also report a Claude settings file that sets `env.YOKE_OFF` or `env.HOME`, because every hook process inherits that environment, an active `yoke pause`, and a session that is open but has recorded no hook decision for 20 minutes.
Remove the `env` entry, run `yoke pause --clear`, or check that session's settings above.

For Codex, `hooks = false` or `codex_hooks = false` under `[features]` in `config.toml` turns hooks off, and `yoke health` reports Yoke hooks missing from `hooks.json`.
Codex runs new or changed hooks only after you approve them with `/hooks`, and it skips unapproved hooks without any warning, including in `codex exec`.
Codex keeps one `[hooks.state."<hooks.json path>:<event>:<group>:<hook>"]` table in `config.toml` per approved hook.
`yoke health` counts those tables and reports `no approval record for N of 7 Yoke hooks`; it can see that a record exists, not whether it is still current or what managed requirements say.
Open Codex, run `/hooks` and approve the Yoke hooks, then start a session and check that `yoke health` shows a Codex event.
To repair hooks that are missing or edited, run `yoke uninstall`, then `yoke install`.
Running `yoke install` again with nothing to change writes nothing and keeps Codex's approvals.
After `yoke uninstall` and `yoke install` with the same executable, Yoke's hook text is identical, but its group goes back at the end of each event's list in `hooks.json`.
Codex records an approval per position and hash, so it keeps Yoke's approval for an event where Yoke's group is still last.
Where another tool added hooks after Yoke's, uninstalling moves those up one position and Yoke returns at the last one, so open `/hooks` and review both again.
This was checked from the files `yoke install` writes and Codex's documentation, not by running Codex.
`yoke install` refuses while a managed hook is edited, with or without `--dry-run`, so there is nothing to preview first.
`yoke extensions` reports each Yoke-managed hook that is missing, edited, unlabeled, or installed for a different `yoke` executable as `stale`.

## `yoke install` says `executable changed`

The installation receipt records the `yoke` executable the hooks run.
Installing from a different path is refused with `executable changed; uninstall before reinstalling`, and the message names both paths and the command to run: `<new yoke> uninstall --agent <agent>`, then install again.
The install script does this for you when you rerun it with the new `YOKE_INSTALL_DIR`; it leaves the old copy on disk and tells you where it is, so remove it if it is still on your `PATH`.
A receipt written for another `CLAUDE_CONFIG_DIR` or `CODEX_HOME` is refused with `settings path differs from installation receipt`, and the message says which variable to set or unset.

## `yoke` is not found outside an interactive shell

The install script adds one line to `~/.bashrc` (and `~/.zshrc` when your `SHELL` is zsh) only if you let it.
Interactive shells read it; cron, scripts and, on Ubuntu, `ssh host command` do not, because Ubuntu's `~/.bashrc` returns early for non-interactive shells above where the line sits.
Use the full path in those places, for example `ssh host '~/.local/bin/yoke health'`, or set `PATH` yourself in the script.
Hooks are not affected: they always run the executable by its absolute path.

## Codex says commands were blocked by the sandbox

If Codex reports that commands or edits "were blocked by the sandbox", run `yoke log --json` in that project and read the `tool_response.excerpt` inside the stored `payload` of the Codex `post_tool_use` row (Yoke keeps the first 200 characters of a shell command's error output, not the whole response).
A message such as `bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted` comes from Codex's own sandbox, not from Yoke: Yoke's decisions for those events are `allow`.
It was seen on Ubuntu 24.04 with `kernel.apparmor_restrict_unprivileged_userns = 1` and no `bubblewrap` package, together with Codex's warning that it could not find bubblewrap on `PATH`.
Installing the distribution's `bubblewrap` package or relaxing that sysctl needs root; the package route is the likely fix but was not verified.
Sandbox prerequisites are described in the [Codex documentation](https://developers.openai.com/codex/concepts/sandboxing#prerequisites).

## A hook error blocked work

Hook failures deny blocking events in enforce mode.
If state cannot be read, a usable record of the project's [last known observe mode](#the-record-of-the-last-known-mode) allows the call with a warning; otherwise an unknown mode is treated as enforce.
Observe mode blocks nothing.
The redacted reason is stored as an `error` decision, or appended to `~/.yoke/error.log` when the database cannot be written.
Read recent errors with `yoke log --outcome error --limit 10`.
If the cause is storage, see [Storage failures block work](#storage-failures-block-work).
[When Yoke itself fails](#when-yoke-itself-fails) lists each failure Yoke was tested against, what the agent and you see, and the way back.
For an explicit temporary bypass, run `yoke pause` in your own terminal and follow what it prints; see [Storage failures block work](#storage-failures-block-work).

## Trust or approval is rejected

`trust`, `console`, `pause`, `jobs retry`, `notifications ack`, `plan approve`, `plan resume`, `plan cancel`, `plan reassign`, `plan accept`, `plan resolve`, and `plan acknowledge` require a human's interactive terminal, as does `plan task done` for a task with `owner = "human"`.
Detected agent tool processes and redirected/noninteractive approval are rejected.
Run the requested command yourself, review the displayed content, and type the exact confirmation phrase.
`install`, `uninstall` and `update` (without `--dry-run` or `--check`) refuse to run from an agent's tool process but need no terminal, so the install script keeps working from your own shell.
If an agent tried to install or remove Yoke for you, run the command yourself.
These safeguards do not replace operating-system isolation; see [Security model and limits](workflows.md#security-model-and-limits).
`plan approve` is also refused while a blocking finding from `yoke plan validate` is open; the error lists each one.
Resolve it with `yoke plan resolve <slug> <id>` after fixing the plan, or accept the risk with `yoke plan acknowledge <slug> <id>`.

## A plan cannot complete

Inspect `yoke plan status <slug>`.
Confirm that its contract was approved, its session exists, and its frozen commands pass in the governed project.
The Git repository must have a HEAD commit; a new repository needs an initial commit, which in turn needs `user.name` and `user.email` set in Git.
A refused plan command names the plan's state and the next command, for example `plan 'x' is draft, but only a building plan can run checks; a human approves it with ...`.
Plans hold a Stop only in enforce mode; in observe mode the Stop is recorded as `would_deny` and nothing is blocked.
Changes during checks invalidate the fingerprint; rerun on a stable final tree.
For required review, verify the selected agent CLI is installed and authenticated and inspect its recorded findings.
A plan waits after the agent runs `yoke plan ask`, and stalls after `stop.max_blocks` Stop attempts without progress (default 5).
Waiting or stalled plans need human resume rather than editing a state label in Markdown: run `yoke plan resume <slug>` in your own terminal, or use Resume work on the console plan page.
A plan stuck in `checking` after its check process was killed also needs `yoke plan resume <slug>`, or Recover interrupted check on the console plan page; both are refused while a live check still owns the plan.
If its header changed after the interruption, recovery records that mismatch without approving it; review and approve the edited header with `yoke plan approve <slug>` before running checks again.
When the plan declares requirements or tasks, the `acceptance` check run lists each unsatisfied requirement and unfinished task with the command that completes it.
Use `yoke plan cancel <slug> --reason '<why>'` to abandon a plan, or `yoke plan reassign <slug> --session <session-id>` to give it to a replacement session.

## Yoke treats a directory as a different project

Run `yoke config` in the directory that behaves differently.
It prints the `project:` Yoke resolved and each `yoke.toml` and `.yoke/knowledge.md` it looked at, with `trusted`, `stale` and `exists`.
[Where Yoke looks for your project](getting-started.md#where-yoke-looks-for-your-project) lists the rules for every layout; the rows below are the surprises.

| What you see | Why and the way back |
| --- | --- |
| Gates work in the main checkout but not in another directory of the same repository | A `yoke.toml` closer to you is not trusted, or the directory was renamed or moved. Trust is keyed to the real path of the project. Run `yoke trust` there, or delete a stray file. `yoke gates` names an ignored file. |
| A new linked worktree has no memory, plans or log from the main checkout | Each worktree is its own project. The policy trusted for the repository still applies in it, so the gates run; only memory, plans and log are separate. |
| A worktree's `yoke.toml` says `stale` | You edited the worktree's copy. The bytes trusted for the repository keep applying, as for any trusted file; run `yoke trust` in the worktree to approve its own bytes. |
| `yoke init` printed a path in another directory | You ran it in a subdirectory, so it wrote the project root's file instead of starting a second project. To make a subdirectory a project of its own, copy `yoke.toml` into it yourself and run `yoke trust` there. |
| Memory, plans or log vanished after renaming or moving the project | They are stored under the old path. Recreate the old directory and run `yoke recall`, or `yoke log --project '<old path>'`, to read them. No command moves them to the new path yet, and `yoke memory import` into the same state refuses records it already holds. Finish or cancel plans before moving a project. |
| Tool calls were denied right after the project directory was renamed or removed | Yoke now applies the policy of the directory the agent reports when the directory the session started in no longer exists, and that of the directory the hook runs in when the reported one is gone too. If an older `yoke` still denies, restart the agent from the new location, or start it with the token that `yoke pause` prints. |
| Hooks run in a directory but nothing is recorded under the project you expect | A session belongs to the project it started in: `CLAUDE_PROJECT_DIR` for Claude Code, else the directory of the session's first event. The decision log records that project, not the directory each tool call reports, so an agent that changes into another repository, or out of the project, stays under the policy of the project it started in. A session keeps its start directory when you resume it elsewhere; start a new session in the directory you want. A directory an agent adds to its workspace is governed by the policy of the session's project. |
| Yoke lists a `yoke.toml` as untrusted (ignored) in a subdirectory of a trusted project | Nobody trusted that file, so it applies nothing and the trusted policy above it stays in force, including when an agent creates the file and changes into its directory. Run `yoke trust` in your own terminal from that directory if you want it, after reading it. |

## Memory import fails or recall looks empty

Recall is project-scoped, and `yoke recall` prints the `project:` path it searched; compare it with the directory where you saved the note.
Inside a directory without git a subdirectory is a separate project; inside a repository the git root is used.
`yoke forget`, `yoke share` and `yoke memory pin` fail with `memory <id> not found in this project` for an ID that belongs to another project.
Run `yoke recall '<query>' --explain` to see which notes match, with their rank and matched words, and what session start would inject.
Imports require explicit absolute-path mappings for all source projects, even unchanged paths; the error prints the `--map '<source>=<source>'` line that restores in place.
Run `yoke memory import <file> --map '<source>=<destination>' --dry-run` before applying it; it prints the text of the memories it would add.
Imported notes come in unpinned; restore your own export's pins with `--keep-pins`, or pin a note after reading it with `yoke memory pin <id>` in your own terminal.
`yoke remember --pinned`, `yoke memory pin` and the real import refuse to run from an agent's tool process (`cannot run from an agent tool process ... run it in your own terminal`); run them in your own terminal.
An import that says `has timestamp ... outside 0 to one day after now` found a record dated before 1970 or more than a day ahead, which an export never writes; do not import that file.
Conflicting records fail atomically and a dry run exits 1 when it finds any; repeat imports of identical records are skipped.
A conflict that says `already stored under project <path>` means the archive was imported under that path before, so map it there or import it into a fresh state.
Export refuses to overwrite an existing archive unless `--overwrite` is supplied.

## Saved notes do not reach the agent

Session start injects notes only in enforce mode.
`yoke recall --explain` prints `injection: inactive` while the mode is observe; set `mode = "enforce"` in `yoke.toml` and run `yoke trust` in your own terminal.
Shared knowledge in `.yoke/knowledge.md` is injected only after `yoke trust` has approved the file, and `yoke forget` of a shared note asks for another `yoke trust` once the entry is removed.
The console's Forget action keeps the shared entry in `.yoke/knowledge.md` so Undo can restore the note; its confirmation tells you to remove that entry by hand and run `yoke trust` in your own terminal if you want to stop sharing it.

## The installer refuses a setting or cannot find a release

- `YOKE_DOWNLOAD_URL must be an https:// URL`: plain `http://` is accepted only for `localhost`, `127.0.0.1` and `[::1]`.
  Serve the mirror over HTTPS, or use a loopback address.
- `the install directory must not contain ...`: `YOKE_INSTALL_DIR` is written into your shell startup file, so a double quote, dollar sign, backtick, backslash or newline would break it.
  Choose another absolute path.
- `could not look up the newest Yoke release, so nothing was built`: a source build needs an exact revision and GitHub's release lookup failed, often a rate limit.
  Rerun later, or set `YOKE_VERSION=<release tag>` or `YOKE_COMMIT=<40-character commit>`.
- `no release is published yet and the newest commit of main could not be found, so nothing was built`: with no release the installer builds the commit `main` points to, asks GitHub's API for it (and `git ls-remote` when git is installed), and neither answered, often because of a rate limit.
  The message prints the command to run with a hash you choose, `curl -fsSL https://raw.githubusercontent.com/mrx-arafat/yoke/main/install.sh | YOKE_COMMIT=<40-character commit hash> bash`; the hashes are at https://github.com/mrx-arafat/yoke/commits/main.
- `invalid YOKE_COMMIT`: it must be a full lowercase commit hash, and it cannot be combined with `YOKE_VERSION`.

## Memory digests do not appear

Digests need `digest = true` under `[memory]` in `~/.yoke/config.toml` or the project's `yoke.toml`, trusted with `yoke trust` in your own terminal; while it is off no job is queued and `yoke memory digest` says how to enable it.
Run `yoke jobs` to see queued, running, failed and dead background jobs with their last error.
A `failed` job retries at the next session start after its `run_after` time; a `dead` job used all 3 attempts or hit a problem that retrying cannot fix, such as `memory.digest` being disabled.
Fix the cause shown in `last_error`, for example an unauthenticated digest CLI, then run `yoke jobs retry <id>` in your own terminal.
A job stuck in `running` after its worker died returns to the queue at the next session start once its 12-minute lease expires.
Worker errors are also appended to `~/.yoke/error.log`.
A line `job N ignored transcript ...` means the digest was given a path outside the project and the agents' transcript directories and went on without it; the digest uses the recorded facts only.
A session that was killed before its end-of-session hook, or whose hook never queued a digest, is digested at the next session start in the same project once it has shown no activity for 30 minutes.
Until then it is treated as still working.

## Old history is still in the database

Retention deletes decisions, facts and the check runs of done or cancelled plans older than `log.retention_days` (90 by default).
Session start only queues the cleanup, at most once an hour, and returns at once; a background worker deletes in short batches, so a session never waits for it.
A very large backlog takes several passes: when one pass runs out of time it queues the next, and the worker picks that up at the next session start.
Run `yoke jobs --all` and look for the kind `retention`: `queued` or `running` means it has not finished, `succeeded` means the last pass completed.
A `failed` job retries with backoff; a `dead` job used all 3 attempts.
`yoke health` then says "Retention cleanup is failing" with the last error, which is also appended to `~/.yoke/error.log`.
Fix the cause, usually a full disk or a locked state database, then run `yoke jobs retry <id>` in your own terminal.
The database file does not shrink after a cleanup: SQLite reuses the freed pages for new history.

## Usage looks frozen or incomplete

Usage is imported from the agent's transcript on Stop, SessionEnd and PreCompact, and an import never decides a hook event.
Codex also imports on start and prompt hooks and at most once per minute after tools complete, so quota warnings can arrive during a turn.
A failed import prints `yoke: usage ingestion failed: <reason>` on the hook's stderr and appends it to `~/.yoke/error.log`; the session's usage stays at its last successful import until a later import succeeds.
A transcript containing a malformed line fails every import from the last good one on, so that session's usage stays incomplete; usage counted by earlier imports stays.
Each import resumes where the previous one of that session stopped, so a long session's Stop reads only what was appended and does not get slower as the transcript grows.
An older Codex sample with an unknown time also triggers a separate bounded replay of available source events to recover its outer event timestamp.
This replay resumes its own checkpoint, fills only matching NULL times and updates day totals without deleting raw rows.
Recovery adds work until the available source is replayed; missing or invalid timestamps, mismatched accounting, tail-only imports and records over 32 MiB stay unknown.
An ended session is not repaired until its transcript is imported again.
Yoke reads a transcript again from its start when the file was truncated, replaced or rewritten, which is why a replaced transcript replaces the stored usage.
A single read of more than 32 MiB does not fail: only its newest 32 MiB is counted, the usage in the skipped part is not, and `yoke usage` marks the session in its coverage column (for example `counted 31.8 MiB of 64.7 MiB`) and adds a note.
That happens for a first import of a transcript larger than 32 MiB, and when more than 32 MiB was appended between two imports; usage counted earlier stays.
Each partly counted session also raises one `usage_partial` notification.
Acknowledging it does not change the totals, and repeated imports do not reopen it while the same condition holds.
A full import after a transcript replacement resolves it.
Keep the original transcript: incremental imports add new usage but cannot recover the bytes already skipped.
Quota resets more than the advertised window plus five minutes after the observation are ignored.
An invalid stored quota record is replaced when the agent next reports valid limits; no manual deletion is needed.
Quota projections need at least a minute of same-reset readings within an hour and a latest reading no older than five minutes.
`usage_limit` warns at `usage.warn_percent` (85 by default), earlier projected exhaustion, and 100%; it never blocks a hook.
Codex's interactive CLI shows quota through hook `systemMessage` output. If absent, check `/hooks` trust and `yoke usage` freshness. Codex 0.164.0 does not include these messages in `exec --json`.
Totals for such a session under-report, most for a session that has run for days.
A Codex rollout whose cumulative token total restarts when Codex begins a new conversation segment, for example after a thread settings change, starts a new accounting segment instead of failing, and the next import catches up.
A restart that neither decreases the total nor is reported as the last turn's usage counts only the difference from the previous total.
A replaced or truncated transcript replaces the stored usage for that session on its next import.

## Storage failures block work

State lives in `~/.yoke/state.db` and its SQLite WAL files.
Normal CLI commands find it through `HOME` in every build and refuse an unset, empty or relative value.
An optimized hook invoked through `yoke run-hook` without `honor-home-env` uses the account home when it already holds Yoke state and differs from `HOME`.
Debug hooks and hooks built with `--features honor-home-env` honour `HOME` instead.
Use a disposable OS account or container for shipped-archive acceptance, which runs optimized hooks.
If a read-only command reports that the state database is empty or incomplete (for example a zero-byte file left by a crash), run `yoke config` from your own terminal or start your agent to initialise it.
Free disk space or repair filesystem permissions from your own terminal.
Preserve the database and WAL files: they contain trusted snapshots, memory, decisions, and plan state.
Do not delete them as a troubleshooting shortcut.

If `yoke health`, `yoke memory export` or another read-only command says the state database is missing `state.db-wal` and `state.db-shm`, another SQLite client (the `sqlite3` shell, a database browser or a backup tool) deleted them when it closed the database last.
After a clean close SQLite has already merged the log into `state.db`, so this is not data loss.
Run `yoke log --limit 1` once from your own terminal to recreate the files, then retry; read-only commands create no files, so they cannot do it themselves.

If a hook or `yoke health` says `state database schema N is newer than supported M`, the database was written by a newer Yoke than the one the hooks run, for example after installing an older release over it.
This is not a disk problem.
Every hook of the older `yoke` fails closed and denies tool calls until you act: run the newer `yoke` again (rerun the install script without `YOKE_VERSION`), or remove the hooks with `yoke uninstall --agent claude,codex` in your own terminal.
To go back to an older Yoke, restore an older copy of `state.db` together with its `-wal` and `-shm` files; see [Upgrade notes](upgrade-notes.md).
The install script checks this before it replaces the executable and stops with nothing changed.

If writable storage fails, Yoke tries to evaluate existing trusted policy read-only.
Audit, memory-fact, retention, and update-check failures produce warnings rather than replacing the policy decision.
Blocking policy errors fail closed in enforce mode or when the mode cannot be determined.
Stop checks requiring atomic plan updates can still block if those updates fail.

For an explicit temporary bypass, run `yoke pause` in your own terminal.
Without options it prints a private token; restart the agent from your own terminal with the token as `YOKE_OFF`:

```sh
YOKE_OFF=<token> codex
# Or:
YOKE_OFF=<token> claude
```

This disables Yoke hooks for that process and its children.
To skip every hook for a limited time without restarting the agent, run `yoke pause --for 30m` (at most 24 hours); end it early with `yoke pause --clear`.
Repair the underlying issue and restart normally to restore protection.
`YOKE_OFF` works only with the token kept in `~/.yoke/off-token`.
`YOKE_OFF=1`, any other value, and a `YOKE_OFF` that a settings file sets are ignored, and each refusal is noted in `~/.yoke/error.log`.
The token is private: Yoke's guard keeps an agent from reading it, and you should not paste it into a settings file or a chat.
Putting `YOKE_OFF` inside an already-blocked tool command cannot bypass the hook that runs before it.
Every honoured skip, every refused one and every pause is written to `~/.yoke/error.log`, and `yoke health` and the statusline show an active pause.

## When Yoke itself fails

Every agent action passes through a Yoke hook, so a fault in Yoke has to be safe, and it has to say how to get past it.
These rules hold for every failure in the table below.

- Observe mode never blocks the agent because of a fault in Yoke. When Yoke cannot read its state it cannot read the mode either, so it uses the last known mode of the project from `~/.yoke/mode-cache`. A project last seen in observe mode is allowed through, with a warning on stderr and a line in `~/.yoke/error.log`. A project last seen in enforce mode, a project with no record, and a record that fails its check are denied for blocking events, and the reason says how to get unblocked.
- Enforce mode fails closed, and the reason names the cause. For a state problem it also names the way back.
- A Stop that Yoke denies only because Yoke failed is denied once. The agent marks the repeated Stop with `stop_hook_active`, and Yoke lets that one through, so an agent cannot loop on a fault it is unable to fix.
- Policy decisions survive storage faults. Audit rows, memory facts and usage numbers for the calls made during the fault can be lost; the hook says so on stderr and in `~/.yoke/error.log` when that file can be written.

In every case you can run `yoke pause` in your own terminal and start the agent with the token it prints to work without Yoke while you repair it; see [Storage failures block work](#storage-failures-block-work).

| Failure | What the agent sees | What happens to your data | The way back |
| --- | --- | --- | --- |
| Hook input that is empty, truncated, not JSON, or nested more than 128 levels deep | Enforce: the call is denied with `yoke error: parse hook JSON object: ...`. Observe: nothing is blocked, and the reason ends `observe mode, so nothing was blocked`. A byte order mark in front of the JSON is accepted. | An `error` decision is recorded. | The sender produced bad input. Update the agent, or the wrapper that calls `yoke run-hook`. |
| Hook input over 1 MiB | Enforce: denied with `hook payload exceeds 1 MiB`; observe: nothing is blocked. Yoke reads and drops up to 64 MiB more, so the agent's write does not fail with a broken pipe. | An `error` decision is recorded. | Ask the agent to use smaller tool inputs, for example a large file written in several edits. |
| Tool input over 32 KiB passed to a check gate | The gate still runs. Its `YOKE_TOOL_INPUT` contains at most 32 KiB, ending with `[YOKE TOOL INPUT TRUNCATED]`, and `YOKE_TOOL_INPUT_TRUNCATED=1` marks the shortened value (`0` otherwise). Matchers, `deny` gates and `rewrite` gates still see the full input. The shortened value may not be valid JSON. The separate 1 MiB hook-payload limit still applies. | Only the check command's environment copy is shortened; this does not deny the call by itself. | If the check needs the complete input, have it reject a truncation flag and ask for smaller tool inputs; never treat a shortened JSON value as complete evidence. |
| The working directory the agent reports is gone, or is not an absolute path | A vanished directory is governed by the policy of the project the session started in, or of the directory the hook runs in when that is gone too. A relative path is denied in enforce mode (`hook cwd must be absolute`) and not blocked in observe mode. | An `error` decision is recorded for the relative path. | Restart the agent in an existing directory. |
| The project directory name is not UTF-8 | Yoke says `project scope unavailable`; trusted parent and global policy still apply, and the path alone does not deny tool calls. | Project plans, memory and accounting are unavailable; Yoke never uses a lossy path as another project's identity. | Rename the project directory using UTF-8 names and restart the agent there. |
| `state.db` is damaged (`file is not a database`, `database disk image is malformed`), or is a directory, pipe or device instead of a file | An observe project is allowed through with a warning (see [the record of the last known mode](#the-record-of-the-last-known-mode)). Any other project is denied for blocking events; the reason says the database is damaged, or is not a regular file, and how to move it aside. A Stop is denied once. A pipe in its place does not make the hook wait. | The file is left as it is. | [Recover a damaged state database](#recover-a-damaged-state-database). |
| `state.db` is an empty file | Yoke creates a new database. Trust approvals were in the old one, so the policy falls back to the defaults (observe, no gates) and nothing is denied. The hook says `state.db was empty` on stderr and in `error.log`. A file under ten seconds old is another hook creating the database and is not reported. | Trust approvals, history, memory and plans that file held are gone. A file left empty by a crash during the very first run held nothing. | Review `yoke.toml` and run `yoke trust` again; `yoke config` shows what is trusted. |
| `state.db` is from a newer Yoke | An observe project is allowed through with a warning. Any other project is denied for blocking events; the reason says this `yoke` is older than the database. | Nothing is changed. | See [Storage failures block work](#storage-failures-block-work). |
| Another process holds a write lock on `state.db` | Each hook waits about 2 s, then reads the trusted policy without writing. The policy decides as usual and the hook warns on stderr. | The audit row, memory facts and usage of those calls are not saved. Nothing is corrupted. | Wait for the other process, or close the tool that holds the database open. |
| Another process holds `state.db` exclusively | After about 2 s an observe project is allowed through with a warning. Any other project is denied for blocking events, with `another process holds ~/.yoke/state.db locked: find it with lsof ~/.yoke/state.db`. | Nothing is changed. | Stop the process `lsof` names. |
| `~/.yoke` is read-only, or the disk is full | The trusted policy still decides, read from the existing database. The hook warns on stderr. | Audit rows, memory facts and usage numbers are not saved; `error.log` cannot be written either. A full disk leaves the database consistent. | Free space or repair permissions in your own terminal. Nothing needs to be repaired in the database. |
| `HOME` is unset, empty, relative, or `~/.yoke` cannot be created | Blocking events are denied in both modes. For `HOME` the reason says to set it to your absolute home directory. These invalid values are refused rather than selecting another home. | Nothing is changed. | Start the agent with `HOME` set, or make `~/.yoke` writable. |
| A leftover or invalid `state.db-wal` or `state.db-shm` | SQLite ignores log frames that do not belong to the database. Nothing is visible. | Nothing is lost. | None needed. Do not delete a `-wal` that holds recent work; see [Storage failures block work](#storage-failures-block-work). |
| A trusted record whose text no longer matches its hash | An observe project is allowed through with a warning. Any other project is denied for blocking events, with `trusted snapshot hash mismatch ... run yoke trust`. | Only that record is affected. | Review the file and run `yoke trust` (`yoke trust --global` for the global file). It replaces the record. |
| The record of the last known mode (`~/.yoke/mode-cache`) is missing, edited, unreadable, a symlink, or writable by others | It counts as no record: with unreadable state the call is denied, and the reason says the record cannot be used. | Nothing else is affected. | None needed. The next hook that reads its state rewrites the file. |
| `yoke.toml` edited, made invalid, grown, replaced by a symlink, directory or pipe, deleted, or made unreadable after you trusted it | The trusted bytes keep governing every event. Session start says the file is `stale (using trusted bytes)`. | Nothing is changed. | Review the file and run `yoke trust`. Yoke refuses to trust a symlink or a file over 1 MiB and says why. |
| The agent ends a hook (timeout, interrupt, `kill`) with SIGTERM, SIGINT or SIGHUP | Yoke stops the gate processes it started and exits at once. | The call is not recorded. SQLite's write-ahead log makes a write that was cut off all-or-nothing. | None needed. |
| The agent kills a hook with SIGKILL | A running gate command can outlive the hook, because SIGKILL cannot be caught. | The database stays consistent. | Find leftover gate commands with `ps` and stop them. Keep gate timeouts short. |
| A hook outlives its time budget | Enforce: denied with `hook evaluation exceeded N ms`; observe: nothing is blocked. Running gates are stopped. The budget is 120 s (25 s for a prompt), longer when your trusted gates need it, never above the agent's own hook timeout. | An `error` decision is recorded. | Lower the timeouts of the slow gate, or fix the command it runs. |
| A gate misbehaves | A gate past its `timeout` is killed with its whole process group and reported as `timed out`. A gate that ignores SIGTERM is still killed. The executor buffers at most 4 KiB per output stream. Checks with trusted `capture_output = true` retain redacted tails in `yoke log --json`; other checks retain only decision metadata and failure reasons. A gate that exits non-zero with no output is reported as `failed (exit N)`. | Nothing is lost. | Fix the gate. A gate that starts a process in a new session (`setsid`) can leave that process running. |
| Many hooks at once | SQLite orders their writes. Each waits up to about 2 s for the write lock. | Every decision is recorded once. | None needed. |
| `error.log` keeps growing | Nothing. Past 1 MiB the file is moved to `error.log.1` and a new one starts; one older generation is kept. | Older entries are dropped when the next rotation replaces `error.log.1`. | Read recent errors with `yoke log --outcome error --limit 10`. |

### The record of the last known mode

Every hook that reads its policy successfully notes the mode (observe or enforce) it found for the project it governs in `~/.yoke/mode-cache`.
The file is written only when a project's mode changed, so an ordinary hook only reads a few lines of it; it holds the newest 256 projects.
It is created with mode 0600 and replaced atomically.
Each line carries a keyed hash of its project, mode and time, so a stray edit or a copy from another home is detected and the whole file is then ignored.
The hash is not a secret: any process running as you can write the file.
The guard stops an agent's tools and shell commands from writing it, like everything else in `~/.yoke`.

The record is read only when the state database cannot be read, and only before the project's policy was loaded.
Observe promises to block nothing, so an observe project is let through with a warning on stderr and a line in `error.log` that names the cause and the way back.
An enforce project is denied, because letting it through would drop its gates.
The record is out of date when the mode changed while the state was already unreadable, and between `yoke trust` and the first hook after it: `yoke trust` does not write the record, the next hook does. It still says what was true when a hook last read the state.

### Recover a damaged state database

Do this in your own terminal. While the database is damaged, blocking events are denied, or you can work without Yoke by starting the agent with the `YOKE_OFF` token that `yoke pause` prints.

1. Move the damaged files aside instead of deleting them: `mv ~/.yoke/state.db ~/.yoke/state.db.damaged`, and the same for `~/.yoke/state.db-wal` and `~/.yoke/state.db-shm` when they exist.
2. Start the agent again, or run `yoke config`. A new database is created.
3. Review `yoke.toml` and `~/.yoke/config.toml`, then run `yoke trust` and `yoke trust --global` again to approve them.
4. Keep the moved copy. Decisions, memory, plans and usage that lived in the damaged file are not in the new database; to get them back, restore a backup of `state.db` together with its `-wal` and `-shm` files.

## Console or updates are unavailable

Start `yoke console` yourself in an interactive terminal; it refuses to run inside an agent tool process.
Open the link it prints within five minutes; each link works only once and signs one browser in with a session cookie.
A page saying the launch link expired or was already used means the link was spent or too old: restart `yoke console` for a new one.
A page saying "This is not the console's address" means the browser reached the console under another host name or port, such as `localhost` instead of `127.0.0.1`, or a different local port in an SSH tunnel; the link is still unused, so open it again at exactly the printed address.
On a server the console is reachable only through an SSH tunnel; see [Console on a remote server](workflows.md#console-on-a-remote-server).
A browser that reports a refused connection has no tunnel to the console's port, or the console is no longer running.
`yoke console` says the port is in use when `--port` names a port another process holds; pick another number or omit `--port`.
If you did not open the link yourself, something else used it; stop the console and check what else can read your terminal.
A link from a previous console process never works for a new instance.

`yoke update` refuses a download that does not run: `the downloaded executable failed its --version check`, or `did not answer --version in time`, means the file is for another platform or damaged, and the installed executable was not changed.
Rerun the installer, which builds from source when no matching executable is released.

`yoke update --check` depends on a published stable GitHub release.
Draft releases and local development changes are not distributed updates.
Until the first release exists, GitHub answers 404 and Yoke says `no stable release is published yet (GitHub answered 404)`; that is a normal state, exits 0 and is not written to `~/.yoke/error.log`.
A rate limit (`GitHub rate limit reached (HTTP 403 or 429)`) clears by itself; unauthenticated requests are limited per IP address, so many machines behind one address share the limit.
`cannot resolve the GitHub host`, `cannot connect to GitHub` and `timed out` mean the network, not the release.
Continue using a source build when no eligible release exists.

## Report a problem

Include the Yoke version, operating system, agent name, failing command, redacted diagnostic, and a minimal policy or payload that reproduces the issue.
Inspect logs before sharing: recognizable-secret masking does not guarantee that all sensitive content was removed.
Do not attach the full state database, memory archive, credentials, or console links.
