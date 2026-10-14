# 0009 Usage history: calendar periods in an explicit UTC offset, no budgets or attribution

Status: accepted on 2026-10-14 for issue #19.

## Context

Usage was grouped by session and model only, and `usage_samples` had no time column (`crates/yoke-core/src/store/004-guard-usage.sql`).
Both transcript formats carry an RFC 3339 `timestamp` on every line, for example `2026-10-14T05:51:23.683Z`, verified on local Claude Code 2.1.299 and codex-cli 0.162.1 files.
Both vendors call the transcript format unstable (see decision 0008), so a missing or malformed time must degrade to unknown rather than fail the import.

## Decision: sample time

Migration 012 adds a nullable `usage_samples.ts` (Unix seconds, UTC) and an index on `(project, ts)` (`crates/yoke-core/src/store/012-usage-time.sql`).
Import keeps a line's timestamp only when it has the exact RFC 3339 shape with `Z` or a `+HH:MM` offset (`crates/yoke-core/src/usage.rs:165`), and SQLite `unixepoch()` converts it on insert.
Bare numbers are rejected because SQLite date functions read them as Julian day numbers (https://sqlite.org/lang_datefunc.html).
A Claude message streamed in several copies keeps the time of its final copy; a Codex delta takes the time of the `token_count` event that produced it.
Rows imported before migration 012 keep no time until their session's transcript is imported again on a later Stop, SessionEnd or PreCompact; sessions that already ended stay "time unknown".

## Decision: time zone semantics

Days and months are calendar periods in one fixed UTC offset that the viewer chooses.
The offset travels explicitly in the URL as `tz=+06:00`, and the page prints it, for example "calendar days in UTC+06:00".
Without `tz`, the console uses the host's current UTC offset, because the console binds loopback only and runs on the viewer's own machine (`crates/yoke-cli/src/console/usage.rs:54`).
An offset must be a 15-minute step between -12:00 and +14:00; anything else is rejected, and an unusual host offset falls back to UTC.
`from` and `to` are inclusive whole days in that offset: a range covers `[from 00:00, to + 1 day 00:00)` local time.
The offset is fixed, so daylight saving rules are not applied: in a zone with daylight saving time, samples within one hour of a transition can land on the neighbouring day when the range spans the transition.
Supporting named IANA zones would need a bundled time zone database dependency, which this change does not justify.
A bounded range leaves out samples without a time and states how many it left out; an unbounded view lists them under "Time unknown".
`usage::tests::history_splits_days_at_the_viewers_midnight_and_bounds_whole_days` pins a sample one millisecond before and one exactly at local midnight in UTC+06:00.

## Decision: unknown prices

Every aggregate (totals, each day or month, each session and model) carries the exact estimate only when every sample is priced, plus the priced part and the number of unpriced samples.
A group with any unpriced sample shows "known part + unknown (N samples)" or "Unknown (N samples)", never a definite number.
`usage::tests::unknown_prices_stay_unknown_in_every_aggregate` pins this.

## Decision: budgets, savings and commit attribution are not added

Budgets: the only spend figure is an estimate from a price table compiled into Yoke (`crates/yoke-core/src/usage.rs`, `claude_cost`), subscription plans are not billed per token, and Codex samples carry no price.
A budget built on that would look precise and be wrong for most users.
Claude Code already has `--max-budget-usd` for print mode, based on its own client-side estimate (https://code.claude.com/docs/en/cli-reference).
Savings: a saving needs a baseline of what would have been spent without Yoke, which Yoke cannot observe.
Commit or shipped-code attribution: Yoke does not record which commits a session produced; only plans record a `base_ref` (`crates/yoke-core/src/store/007-plan-base-ref.sql`).
Linking spend to commits would need time-window heuristics over Git history that misattribute concurrent sessions and rewritten commits.
None of these is added.
Yoke never presents commit counts or spend as evidence of software quality; checks and independent review findings remain the quality evidence.
Any later attribution feature must name its data source and show its limitations on the same page.
