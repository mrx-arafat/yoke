# Troubleshooting

[Documentation index](README.md)

Yoke records actionable problems as notifications: hook errors, would-deny summaries in observe mode, waiting or stalled plans, failed or interrupted checks, and stale or untrusted configuration.
Read them with `yoke notifications` or on the console Notifications page, which also has a searchable copy of the recovery steps below.
Each notification names the section of this page that explains it.

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
The nearest project configuration wins discovery; inspect origins when running from nested directories.

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

## Configuration changed after it was trusted

Yoke keeps enforcing the exact bytes a human trusted.
An edited, deleted or unreadable file on disk has no effect until new bytes are trusted.
Compare trusted and disk content on the console Trust & config page, or look for `stale` in `yoke config`.
If the edit is intended, run `yoke trust` (or `yoke trust --global` for `~/.yoke/config.toml`) in your own terminal.
If it is not intended, restore the file.
A deleted file cannot be trusted; the trusted snapshot keeps applying meanwhile.

## Configuration is not trusted

A `yoke.toml`, `~/.yoke/config.toml` or `.yoke/knowledge.md` without a trusted snapshot contributes no settings, gates or knowledge.
`yoke init` creates a starter file but does not trust it.
Review the file, then run `yoke trust` (or `yoke trust --global`) in your own terminal.
Project knowledge is trusted together with the project `yoke.toml`, which must exist.
Agents cannot trust configuration.

## Hooks are disabled or missing

Yoke protects nothing when its hooks do not run.
Run `yoke health` to see which agents have Yoke hooks installed and what stops them.
An unreadable installation receipt is reported as a problem rather than a missing installation; repair the reported receipt before retrying installation.
An empty or zero-byte state database is reported as empty or incomplete without hiding installed agents; initialize it from your own terminal with `yoke config`, or let the next agent hook initialize it.
Yoke reads Claude Code settings from `CLAUDE_CONFIG_DIR` and Codex settings from `CODEX_HOME` when they are set to absolute paths, and from `~/.claude` and `~/.codex` otherwise.
Codex's top-level `allow_managed_hooks_only = true` also prevents Yoke's user hooks from running; `yoke health` reports the same blocker as installation.

For Claude Code, the highest-precedence file that sets `disableAllHooks` decides: the project `.claude/settings.local.json`, then the project `.claude/settings.json`, then the user `settings.json`.
If that file sets it to `true`, Yoke's hooks do not run; remove the key or set it to `false` there.
`disableAllHooks` or `allowManagedHooksOnly` set to `true` in file-based managed settings also stops them: `managed-settings.json` or `managed-settings.d/*.json` in `/Library/Application Support/ClaudeCode` on macOS and `/etc/claude-code` elsewhere.
`allowManagedHooksOnly` has effect only in managed settings ([settings reference](https://code.claude.com/docs/en/settings-reference.md), [managed settings](https://code.claude.com/docs/en/managed-settings.md)), so only an administrator can change it.
Managed policy delivered by MDM or the claude.ai console is not visible to Yoke.

For Codex, `hooks = false` or `codex_hooks = false` under `[features]` in `config.toml` turns hooks off, and `yoke health` reports Yoke hooks missing from `hooks.json`.
Codex runs new or changed hooks only after you trust them with `/hooks`; Yoke cannot read that approval or managed requirements.
Preview a reinstall with `yoke install --dry-run`, then run `yoke install`.
`yoke extensions` reports each Yoke-managed hook that is missing, edited, unlabeled, or installed for a different `yoke` executable as `stale`.

## A hook error blocked work

When a Yoke hook fails in enforce mode, or when the mode cannot be read, Yoke fails closed and denies blocking events instead of allowing them.
The redacted reason is stored as an `error` decision, or appended to `~/.yoke/error.log` when the database cannot be written.
Read recent errors with `yoke log --outcome error --limit 10`.
If the cause is storage, see [Storage failures block work](#storage-failures-block-work).
For an explicit temporary bypass, restart the agent from your own terminal with `YOKE_OFF=1`.

## Trust or approval is rejected

`trust`, `console`, `jobs retry`, `notifications ack`, `plan approve`, `plan resume`, `plan cancel`, `plan reassign`, `plan accept`, `plan resolve`, and `plan acknowledge` require a human's interactive terminal, as does `plan task done` for a task with `owner = "human"`.
Detected agent tool processes and redirected/noninteractive approval are rejected.
Run the requested command yourself, review the displayed content, and type the exact confirmation phrase.
These safeguards do not replace operating-system isolation; see [Security model and limits](workflows.md#security-model-and-limits).
`plan approve` is also refused while a blocking finding from `yoke plan validate` is open; the error lists each one.
Resolve it with `yoke plan resolve <slug> <id>` after fixing the plan, or accept the risk with `yoke plan acknowledge <slug> <id>`.

## A plan cannot complete

Inspect `yoke plan status <slug>`.
Confirm that its contract was approved, its session exists, and its frozen commands pass in the governed project.
The Git repository must have a HEAD commit.
Changes during checks invalidate the fingerprint; rerun on a stable final tree.
For required review, verify the selected agent CLI is installed and authenticated and inspect its recorded findings.
A plan waits after the agent runs `yoke plan ask`, and stalls after `stop.max_blocks` Stop attempts without progress (default 5).
Waiting or stalled plans need human resume rather than editing a state label in Markdown: run `yoke plan resume <slug>` in your own terminal, or use Resume work on the console plan page.
A plan stuck in `checking` after its check process was killed also needs `yoke plan resume <slug>`, or Recover interrupted check on the console plan page; both are refused while a live check still owns the plan.
If its header changed after the interruption, recovery records that mismatch without approving it; review and approve the edited header with `yoke plan approve <slug>` before running checks again.
When the plan declares requirements or tasks, the `acceptance` check run lists each unsatisfied requirement and unfinished task with the command that completes it.
Use `yoke plan cancel <slug> --reason '<why>'` to abandon a plan, or `yoke plan reassign <slug> --session <session-id>` to give it to a replacement session.

## Memory import fails or recall looks empty

Recall is project-scoped; inspect the resolved project using `yoke config`.
Run `yoke recall '<query>' --explain` to see which notes match, with their rank and matched words, and what session start would inject.
Imports require explicit absolute-path mappings for all source projects, even unchanged paths.
Run `yoke memory import <file> --map '<source>=<destination>' --dry-run` before applying it.
Conflicting records fail atomically; repeat imports of identical records are skipped.
Export refuses to overwrite an existing archive unless `--overwrite` is supplied.

## Memory digests do not appear

Run `yoke jobs` to see queued, running, failed and dead background jobs with their last error.
A `failed` job retries at the next session start after its `run_after` time; a `dead` job used all 3 attempts or hit a problem that retrying cannot fix, such as `memory.digest` being disabled.
Fix the cause shown in `last_error`, for example an unauthenticated digest CLI, then run `yoke jobs retry <id>` in your own terminal.
A job stuck in `running` after its worker died returns to the queue at the next session start once its 12-minute lease expires.
Worker errors are also appended to `~/.yoke/error.log`.
A session that was killed before its end-of-session hook, or whose hook never queued a digest, is digested at the next session start in the same project once it has shown no activity for 30 minutes.
Until then it is treated as still working.

## Usage looks frozen or incomplete

Usage is imported from the agent's transcript on Stop, SessionEnd and PreCompact, and an import never decides a hook event.
A failed import prints `yoke: usage ingestion failed: <reason>` on the hook's stderr and appends it to `~/.yoke/error.log`; the session's usage stays at its last successful import until a later import succeeds.
A transcript over 32 MiB or containing malformed JSON fails every import, so that session's usage stays incomplete.
A Codex rollout whose cumulative token total restarts when Codex begins a new conversation segment, for example after a thread settings change, starts a new accounting segment instead of failing, and the next import catches up.
A restart that neither decreases the total nor is reported as the last turn's usage counts only the difference from the previous total.
A replaced or truncated transcript replaces the stored usage for that session on its next import.

## Storage failures block work

State lives in `~/.yoke/state.db` and its SQLite WAL files.
Yoke finds it through `HOME`: it refuses an unset, empty or relative `HOME` and never falls back to another home directory.
If a read-only command reports that the state database is empty or incomplete (for example a zero-byte file left by a crash), run `yoke config` from your own terminal or start your agent to initialise it.
Free disk space or repair filesystem permissions from your own terminal.
Preserve the database and WAL files: they contain trusted snapshots, memory, decisions, and plan state.
Do not delete them as a troubleshooting shortcut.

If writable storage fails, Yoke tries to evaluate existing trusted policy read-only.
Audit, memory-fact, retention, and update-check failures produce warnings rather than replacing the policy decision.
Blocking policy errors fail closed in enforce mode or when the mode cannot be determined.
Stop checks requiring atomic plan updates can still block if those updates fail.

For an explicit temporary bypass, restart the agent from your own terminal:

```sh
YOKE_OFF=1 codex
# Or:
YOKE_OFF=1 claude
```

This disables Yoke hooks for that process and its children.
Repair the underlying issue and restart normally to restore protection.
Putting `YOKE_OFF=1` inside an already-blocked tool command cannot bypass the hook that runs before it.

## Console or updates are unavailable

Start `yoke console` yourself in an interactive terminal; it refuses to run inside an agent tool process.
Open the link it prints within five minutes; each link works only once and signs one browser in with a session cookie.
A page saying the launch link expired or was already used means the link was spent or too old: restart `yoke console` for a new one.
If you did not open the link yourself, something else used it; stop the console and check what else can read your terminal.
A link from a previous console process never works for a new instance.

`yoke update --check` depends on a published stable GitHub release.
Draft releases and local development changes are not distributed updates.
Continue using a source build when no eligible release exists.

## Report a problem

Include the Yoke version, operating system, agent name, failing command, redacted diagnostic, and a minimal policy or payload that reproduces the issue.
Inspect logs before sharing: recognizable-secret masking does not guarantee that all sensitive content was removed.
Do not attach the full state database, memory archive, credentials, or console links.
