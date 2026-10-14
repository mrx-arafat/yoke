# 0007 Trial evidence and the working-day definition

Status: accepted on 2026-10-14 for issue #22 (trial criteria only).

## Context

Issue #22 requires a full-working-day observe trial and a week of real work in enforce mode, for Claude Code and Codex, with unexpected denials resolved and recovery incidents documented.
The design roadmap names the same two trials (`docs/specs/2026-10-13-yoke-design.md`, roadmap rows 0.2 and 1.0) without defining a working day.
Every hook event already writes one decision row with its time, agent, mode and outcome (`crates/yoke-core/src/store.rs`, `record_decision_in`), and plan recovery is already audited in `transitions` and `check_runs` (`crates/yoke-core/src/plan/storage.rs`, `update`, `recover`).
Without a definition and a tool, a human has to count hours and denials by hand, and calendar span alone overstates a trial: the first real Codex observe run spans 19.5 hours but has only 6 active hours.

## Decision

A working day is a local calendar day on which an agent, in one mode, has decisions in at least 6 distinct clock hours (`crates/yoke-core/src/trial.rs:13`).
The observe trial needs 1 working day per agent and the enforce trial 5, matching "a full working day" and "a week of real work" on a five-day week (`trial.rs:15`).
Clock-hour buckets measure presence of work, not effort: an hour with one event counts the same as an hour with a thousand.
The local time zone decides calendar days, because a working day is a human's day; SQLite's `localtime` modifier supplies it without a new dependency (`trial.rs`, `modifiers`).

`yoke trial report` derives everything from existing records and opens the store with `Store::open_read_only`, so it never creates, migrates or writes state (`crates/yoke-cli/src/trial.rs:52`).
No migration or new table is needed.
Classifying a flagged decision as expected or unexpected, and resolving it, stays a human judgment recorded in `docs/trials.md`; the report lists what is pending and marks that criterion `needs_human` until nothing is flagged.
Recovery incidents are plan resumes from `waiting`, `stalled` or `checking`, stalls, cancellations, reassignments, interrupted check runs and `~/.yoke/error.log` lines.

Synthetic soak runs (`crates/yoke-cli/tests/soak.rs`) find defects and measure latency, but compressed time never produces a working day, and they never count toward either trial.

## Alternatives considered

- Calendar span from first to last event: rejected, because idle nights count as work.
- Session duration: rejected, because an idle session can stay open (Codex documents SessionEnd after 30 idle minutes without a connected client, [hooks](https://learn.chatgpt.com/docs/hooks.md), "SessionEnd"), and a working day often spans several sessions; the real Codex run had 17 sessions in 6 active hours.
- Hand-written trial logs only: rejected as the source of numbers, because they drift from what hooks recorded; they remain the place for human classification.

## Consequences

A trial day with fewer than 6 active hours does not count, even when work was real.
Plan incidents cannot be attributed to one agent, because plans belong to a project.
Background update and job workers write error.log lines without a timestamp (`crates/yoke-cli/src/hook.rs:466`, `crates/yoke-cli/src/jobs.rs:104`), so the report cannot place them in a window and lists them with time `unknown`.
