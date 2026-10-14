# 0006 Plan requirements, tasks, and pre-approval feedback

Status: accepted on 2026-10-14 for issues #15 and #16.

## Context

Before this change a plan header held only `title`, `review` and `[[check]]` entries, and state was plan-level (`crates/yoke-core/src/plan.rs`, `store/003-workflow.sql`).
Approval freezes the exact header bytes, and `files::approved` refuses to run checks when the header on disk differs (`crates/yoke-core/src/plan/files.rs:155`).
Independent review ran only at `yoke plan check` time, and human questions could only pause the whole plan.

## Decision

Requirements and tasks live in the TOML header as `[[requirement]]` and `[[task]]` arrays, so the existing frozen approval covers them without a new trust mechanism.
Validation happens at parse time: ids are unique and use `[a-z0-9-]`, references must exist, and Kahn's algorithm rejects dependency cycles (`crates/yoke-core/src/plan/acceptance.rs:139`).
A requirement is satisfied either by named blocking checks (or `review`) or by human evidence, never both (`acceptance.rs:168`).
Linking only blocking checks keeps `blocking = false` meaning "never decides the outcome".
Completion is runtime data in `plan_completions` (migration 009), keyed by plan, kind, item id, and the SHA-256 of the approved item definition, so evidence for an edited item stops counting after re-approval (`acceptance.rs:242`).

Dependency behaviour is the least that is enforceable: a task cannot be marked done before every task in its `depends_on`, and status reports each task as `done`, `ready` or `blocked`.
Ownership has two values: `agent` (default) and `human`.
A human-owned task refuses any actor other than an authenticated human, using the same `require_human` and confirmation pattern as `plan approve` (`acceptance.rs:446`).
Completing an agent-owned task is recorded as an unverified agent assertion; its value is explicit tracking, and the real proof remains the checks.
There is no assignee, estimate, or undo, because nothing in Yoke consumes them.

When a header declares requirements or tasks, `yoke plan check` records an `acceptance` run, and `done` additionally requires every requirement satisfied and every task done (`crates/yoke-core/src/plan/checks.rs:71`).
The check name `acceptance` is reserved only when either section is present, so existing approved headers keep parsing.

Pre-approval review reuses the configured reviewer selection and headless command (`review::command`), with a plan prompt and a strict findings contract of `severity` (`blocking` or `advice`), `anchor` and `summary`.
Findings and human comments share one `plan_feedback` table with open, resolved, or acknowledged state; only a human closes an item.
Approval is refused while any blocking finding is open, regardless of the plan revision it reviewed, and the check runs inside the approving transaction (`crates/yoke-core/src/plan.rs:293`).
Ignoring stale findings would let an agent hide a blocking finding by editing one byte of the plan.
Anchors are prefixed (`requirement:`, `task:`, `check:`, `heading:`, or `plan`) so ids and heading slugs cannot collide, and unknown anchors are rejected.

## Alternatives rejected

Parsing tasks from Markdown checkboxes was rejected because headings and checkboxes are freeform and are not covered by the frozen header.
Making validation mandatory before approval was rejected for now: it would require a reviewer CLI for every plan, and the blocking-finding gate already prevents approving over known defects.
Letting stale blocking findings lapse automatically was rejected for the bypass reason above.
