# 0008 Session continuation: copyable resume commands, no transcript viewer

Status: accepted on 2026-10-14 for issue #14.

## Context

The console used to scope every page to the single project it was launched from.
Yoke records sessions (`id`, `agent`, `project`, `started_at`, `ended_at`, `active_plan`, `decision_count`) in `sessions` (`crates/yoke-core/src/store/001-core.sql`, `003-workflow.sql`, `006-session-counts.sql`).
Yoke does not store transcripts, and it has no column for transcript paths.
The raw hook payload, which can contain `transcript_path`, is kept only inside `decisions.payload` after `redact_payload` redacts and caps it (`crates/yoke-cli/src/hook.rs:107`, `crates/yoke-core/src/store.rs:238`).
Usage import reads a transcript at Stop, SessionEnd and PreCompact and persists only counters, model names, estimated prices and line timestamps, never message content (`crates/yoke-core/src/usage.rs`, `ingest`).
Claude Code describes its JSONL transcript entries as internal and liable to change between versions (https://code.claude.com/docs/en/sessions#where-transcripts-are-stored).
Codex says the transcript format "isn't a stable interface for hooks and may change over time" (https://developers.openai.com/codex/hooks).

## Decision: transcript access

Yoke does not add a conversation or transcript viewer.
Transcripts hold prompts, file contents and tool output, and Yoke's redaction is pattern based, so it cannot promise that a rendered transcript is free of secrets.
A viewer would also widen what a leaked console token exposes, and it would depend on two formats both vendors call unstable.
Both agents already offer native export: `/export` in Claude Code (https://code.claude.com/docs/en/sessions) and `/export` in Codex (https://developers.openai.com/codex/cli/reference).
The console may show session identity, lifecycle, newest decision time, decision count, plan binding, the mode Yoke applied at the newest decision, the project's current trust state, and a resume command.
It shows no transcript content and no transcript path outside the existing redacted payload on the Decisions page.
A future viewer would need, at minimum, per-project opt-in, reading through a documented vendor interface rather than raw JSONL, redaction, size caps, no persistence, and an audit record of each view.

## Decision: what resume means

Claude Code: `claude --resume <session-id>` resumes a saved conversation by id, searching the current project and then every other project on the machine (https://code.claude.com/docs/en/sessions#resume-by-session-id-or-name).
`claude --continue` loads the most recent conversation in the current directory (https://code.claude.com/docs/en/cli-reference).
The installed Claude Code 2.1.299 help lists `-r, --resume [value]` and `-c, --continue`.
Codex: `codex resume [SESSION_ID]` continues an interactive session by id, `--last` picks the most recent one, and Codex asks which directory to use when the current directory differs from the session's saved one (https://developers.openai.com/codex/cli/reference).
The installed codex-cli 0.162.1 help describes the argument as "Session id (UUID) or session name".
Codex hook input documents `session_id` as the current Codex session id (https://developers.openai.com/codex/hooks).
Yoke already depends on the hook session id matching the transcript's own id: import refuses a Claude `sessionId` or Codex `session_meta.payload.id` that differs (`crates/yoke-core/src/usage.rs`, `read_snapshot`).
The Sessions page therefore shows `cd '<project>' && claude --resume '<id>'` or `cd '<project>' && codex resume '<id>'` as text to copy.
Yoke never executes it and offers no resume button.
The command appears only when the session id matches `[A-Za-z0-9._:-]{1,128}` and the project path has no control characters; both values are single-quoted for POSIX shells (`crates/yoke-cli/src/console/projects.rs:148`).
Other agents get no command.

Known limits:
A session without an end event may still be running; Yoke has no liveness signal and says so on the page.
Claude Code documents that resuming one session in two terminals without forking interleaves both conversations, and that resuming a running background session attaches to it (https://code.claude.com/docs/en/sessions).
Codex behaviour for a session that is open elsewhere is not documented and stays unknown.
Yoke records the project root, not the exact working directory, so Codex may still ask which directory to use.

## Decision: project selection and protection fields

The project list is every distinct `project` in `sessions`, `decisions` and `plans` (`crates/yoke-core/src/store/read.rs:131`).
A page selects a project with `project=<first 16 hex characters of SHA-256 of the recorded path>`; the server maps the id back to a recorded path and rejects anything else (`crates/yoke-cli/src/console/projects.rs:41`).
A client-supplied path never reaches the database query or the filesystem.
Decisions, Sessions and Usage accept the selection; Plans, Memory and Trust stay on the launch project because they read plan files and run mutations scoped to it (`crates/yoke-cli/src/console/actions.rs`).
"Protection now" calls `config::load_trusted` for the recorded root, which compares hashes of the fixed files `yoke.toml`, `~/.yoke/config.toml` and `.yoke/knowledge.md` and never displays their content.
Sessions show lifecycle, last event, plan binding, mode at last event and resume as separate fields, so an open session is never presented as a protected one.
Mode at last event is history from `decisions.mode`; protection now is the current trusted configuration; neither claims the agent is alive.
