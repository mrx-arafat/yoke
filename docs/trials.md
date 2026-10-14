# Trials

[Documentation index](README.md) · [Release acceptance](release-acceptance.md)

Release acceptance (issue #22) needs two trials of real work for each supported agent, Claude Code and Codex:

1. A full working day in observe mode, with every unexpected denial or error resolved.
2. A week of real work in enforce mode, with every recovery incident documented.

Only real work counts.
Synthetic or replayed runs, including the [synthetic soak trials](#synthetic-soak-trials), never satisfy either trial.

## What counts

`yoke trial report` applies these definitions to the decisions Yoke already records:

| Term | Definition |
| --- | --- |
| Active hour | A clock hour in the machine's local time zone with at least one decision for that agent and mode |
| Working day | A local calendar day with at least 6 active hours |
| Observe trial | At least 1 working day in observe mode, per agent |
| Enforce trial | At least 5 working days in enforce mode, per agent |
| Flagged decision | A `deny`, `would_deny` or `error` decision |
| Recovery incident | A plan resumed from `waiting`, `stalled` or `checking`, a plan that stalled, was cancelled or reassigned, an interrupted check run, or an `~/.yoke/error.log` line |

A flagged decision is expected when a gate did its job, and unexpected when it is a false positive or a Yoke fault.
Every flagged decision and error.log line must be classified here, and every unexpected one resolved by a fix or an explicit, reasoned acceptance.
Every recovery incident must be described here, including how it was recovered.

## Collect the evidence

1. Install the hooks and trust the policy for the trial mode, as in [getting started](getting-started.md).
2. Work normally for the whole trial; do not change the mode mid-trial.
3. Run the report in your own terminal, with the trial's first day as the start:

```sh
yoke trial report --since 2026-10-20
yoke trial report --since 2026-10-20 --until 2026-10-27 --agent codex --json
```

The report opens `~/.yoke/state.db` read-only.
It never creates, migrates or writes state, so it is safe to run while hooks are active.
`--since` is inclusive and `--until` exclusive; each takes `YYYY-MM-DD` (local midnight) or an RFC 3339 time with an offset, such as `2026-10-20T09:00:00+06:00`.
It prints, per agent and mode, the calendar span, sessions, active days and hours, working days and outcome counts per event.
It then lists flagged decisions newest first with their failing gates and redacted reasons, recovery incidents, and the acceptance status of each criterion.
Flagged decisions are paged; `--limit` sets the page size (default 100, at most 1000) and `--before <id>` continues from the `next_before` the previous page printed.

Limits of the report:

- Plans are shared by every agent in a project, so plan incidents are not filtered by `--agent`.
- Every error.log line now starts with a timestamp, including those of background update and job workers.
  Lines written by earlier builds may lack it; the report lists those with time `unknown` in every window.
- Only the last 4 MiB of error.log are read.
- A copied WAL database also needs its `-wal` and `-shm` files to open read-only.

The built-in self-protection guard denies any agent shell command that mentions `state.db` ([design 4.5](specs/2026-10-13-yoke-design.md#45-self-protection)).
Agents that need trial evidence should run `yoke log --json` or `yoke trial report` instead of opening the database.

## Evidence record

### Codex, observe mode, real work, 2026-10-13 to 2026-10-14

Codex observe mode, real work, 0.8 day (partial - criterion needs one full working day).

These aggregates come from `yoke trial report` run against a read-only copy of the maintainer's state database on 2026-10-14.
No payloads, commands, paths, prompts or session ids are recorded here.

| Measure | Value |
| --- | --- |
| Decisions | 5,015, all Codex, all observe mode |
| Sessions | 17 |
| Calendar span | 19.5 hours across 2 calendar days |
| Active hours | 6 (4 on the first day, 2 on the second) |
| Working days | 0 of the 1 needed |
| Claude Code decisions | none |
| Enforce-mode decisions | none |
| Plans and recovery incidents | none |

| Event | allow | would_deny | error |
| --- | --- | --- | --- |
| `pre_tool_use` | 2,473 | 10 | 0 |
| `post_tool_use` | 2,255 | 0 | 0 |
| `user_prompt_submit` | 184 | 0 | 0 |
| `stop` | 53 | 0 | 0 |
| `session_start` | 20 | 0 | 0 |
| `session_end` | 12 | 0 | 0 |
| `pre_compact` | 8 | 0 | 0 |

Classification of every flagged decision and error.log line:

| What | Gate or source | Class | Resolution |
| --- | --- | --- | --- |
| 10 `pre_tool_use` `Bash` would_deny decisions, 5 on each day | `self-protection`: "agent shell access to state.db is protected" | Expected | Each command referenced `state.db` (a file listing, read-only `sqlite3` queries, a read-only backup), which design section 4.5 denies by intent. Enforce mode would block them; agents use `yoke log --json` or `yoke trial report` instead. No code change. |
| 1 error.log line: "update download failed (exit status: 56)" | Background `yoke update --check` | Expected | curl exit 56 is a network receive failure, and the repository had no published release to find. It is advisory and affected no gate decision. The line was written by a build from before error.log lines were timestamped, so its time is unknown. |

Status: observe trial not met for either agent; enforce trial not started.

## Templates for the real trials

Copy these tables under a new heading per trial and fill them from `yoke trial report`.

Trial log:

| Agent | Mode | First day | Last day | Working days | Acceptance status from the report | Recorded by |
| --- | --- | --- | --- | --- | --- | --- |

Flagged decisions and error.log lines:

| Decision id or log time | Agent | Event | Gate | Class (expected or unexpected) | Resolution |
| --- | --- | --- | --- | --- | --- |

Recovery incidents:

| Time | Kind | Plan | What happened | How it was recovered | Follow-up |
| --- | --- | --- | --- | --- | --- |

## Synthetic soak trials

Synthetic soak trial - does not replace the real-work trial.
`crates/yoke-cli/tests/soak.rs` drives the real `yoke` binary through a compressed observe day and enforce week for both agents, with injected faults, and fails on any outcome that differs from its script.
It exists to find defects before the real trials.

```sh
cargo test -p yoke-cli --test soak -- --ignored --nocapture --test-threads=1
```

Set `YOKE_SOAK_BIN` to another yoke build, such as a release build in a temporary target directory, for latency numbers, and `YOKE_SOAK_OUT` to a directory to receive each scenario's results as JSON.
Results: [synthetic-2026-10-14](trials/synthetic-2026-10-14.md).

The soak payloads follow the agents' published hook input contracts:

- Claude Code [hooks reference](https://code.claude.com/docs/en/hooks.md), sections "Common input fields" and each event's input.
- Codex [hooks](https://learn.chatgpt.com/docs/hooks.md) (`https://developers.openai.com/codex/hooks` redirects there), sections "Common input fields" and each event, plus the [input schemas](https://github.com/openai/codex/tree/main/codex-rs/hooks/schema/generated) that page links.
- The key structure, without values, of the real Codex payloads recorded in the observe trial above, which shows that Codex sends `tool_response` as a string and `apply_patch` input in `tool_input.command`.
