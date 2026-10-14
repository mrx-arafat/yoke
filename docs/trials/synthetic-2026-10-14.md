# Synthetic soak trial, 2026-10-14

Synthetic soak trial - does not replace the real-work trial.

[Trials](../trials.md)

These runs drive the real `yoke` binary with hook payloads shaped like the ones Claude Code and Codex send, in compressed time, and compare every outcome with a scripted expectation.
They find defects before the real trials; they are not evidence of real use and satisfy no trial criterion.

## Environment

| Item | Value |
| --- | --- |
| OS | macOS 26.5.1 (Darwin 25.5.0), Apple M4, 10 cores, 16 GiB |
| Toolchain | rustc 1.95.0 |
| Commit | `e8b8ee0` plus uncommitted work (dirty tree) |
| Binary for latency | `cargo build --release -p yoke-cli` into a temporary target directory |
| Binary for functional runs | the test profile build (`CARGO_BIN_EXE_yoke`) |
| Command | `cargo test -p yoke-cli --test soak -- --ignored --nocapture --test-threads=1` |

Every run used a temporary home directory and temporary Git projects.
No real agent settings or state were read or written, update checks were pre-claimed so nothing reached the network, and memory digests used an offline stand-in for the agent CLI.

## Scenarios

**Observe day** (344 hook events): both agents, a morning and an afternoon session each, eight compressed working hours of prompts, `Bash`, `Edit`, `Write` and `Read` calls (Codex: `Bash` and `apply_patch`), stops, an automatic compaction and session ends.
A Claude notification, a force-push attempt and a shell command that mentions `state.db` are included.
A plan goes through create, human approval, start, `plan check` and done while Stop is reported as `would_deny`.

**Enforce week** (2,590 hook events): five compressed workdays per agent in enforce mode with a trusted policy that denies force pushes, rewrites `cargo test` to `cargo test --quiet`, checks for `README.md` on Stop, and enables memory digests.
Each day ends with a rewrite, a gate denial and a self-protection denial.
Injected faults:

| Day | Fault | Observed result |
| --- | --- | --- |
| 1, Codex | Plan stops twice without progress | Two Stop blocks, then the plan stalled and the agent could stop; after a human resume, one block, then `plan check` passed and Stop was allowed |
| 2, Claude | `plan check` killed while its check runs | Stop blocked while the plan stayed `checking`; `plan resume` recorded an interrupted check and returned it to building; a new check passed |
| 2, Codex | Transcript with a line over 1 MiB, then one over 32 MiB | Stop, both PreCompact events and the SessionEnd were allowed, each with a `usage ingestion failed` warning |
| 3, Claude | State database, WAL and shared-memory files read-only | Trusted decisions still applied (a force push was denied), nothing was persisted, and each lost write was logged to error.log |
| 3, Codex | Policy file edited after trust; a second project with an untrusted policy | The trusted bytes kept enforcing with a stale notice; the untrusted policy was ignored with a notice |
| 4, both | Two sessions on one project at the same time, 60 events each | All 120 decisions persisted as `allow` with no diagnostics |
| 4, Codex | One session moves to another project | The other project's trusted gate applied, and the session record moved with it |
| 5, Codex | One session with 1,005 memory facts | All 1,005 facts were digested through chained background jobs, with no failed or dead job |

`yoke trial report` ran at the end of each scenario.
It counted exactly the scripted flagged decisions in the observe day (7), listed the stall, resumes, interrupted check and error.log lines of the enforce week, and reported every working-day criterion as `not_met`, because compressed time never spans real working hours.

## Mismatches

None: 2,934 of 2,934 hook outcomes and every plan, usage and persistence check matched the script.
A deliberate wrong expectation was detected, which shows the comparison is live.

Design observation, not a mismatch: a transcript over the usage bounds is never imported, so that session's usage is missing, but every hook event only warns and keeps its policy decision.
An earlier build turned every PreCompact and SessionEnd of such a session into an `error` decision; the soak scenario was updated when that was fixed, and it still passes 2,934 of 2,934 outcomes.
On the development machine, 9 of 795 Claude Code and 4 of 645 Codex transcripts exceed 32 MiB, so long real sessions will produce `usage ingestion failed` warnings in error.log during the trials.

## Hook latency, release build

Wall time from spawning `yoke run-hook` to its exit, in milliseconds.
The first exec of a freshly built binary took about 1.1 s once, before any hook ran, and is excluded by a warm-up call.

Enforce week:

| Agent and event | n | p50 | p95 | p99 | max |
| --- | --- | --- | --- | --- | --- |
| claude PreToolUse | 305 | 4.0 | 4.5 | 5.2 | 8.8 |
| claude PostToolUse | 294 | 3.9 | 4.6 | 5.5 | 9.1 |
| claude UserPromptSubmit | 38 | 3.9 | 4.6 | 6.4 | 6.4 |
| claude Stop | 38 | 10.6 | 59.9 | 62.1 | 62.1 |
| claude SessionStart | 5 | 4.0 | 10.7 | 10.7 | 10.7 |
| claude SessionEnd | 5 | 4.4 | 8.3 | 8.3 | 8.3 |
| claude PreCompact | 5 | 4.1 | 4.5 | 4.5 | 4.5 |
| codex PreToolUse | 336 | 4.0 | 4.9 | 7.4 | 10.7 |
| codex PostToolUse | 1,329 | 4.0 | 6.8 | 17.6 | 62.5 |
| codex UserPromptSubmit | 47 | 3.9 | 6.0 | 6.6 | 6.6 |
| codex Stop | 47 | 10.7 | 61.8 | 67.4 | 67.4 |
| codex SessionStart | 8 | 3.9 | 5.4 | 5.4 | 5.4 |
| codex SessionEnd | 6 | 4.6 | 18.1 | 18.1 | 18.1 |
| codex PreCompact | 7 | 4.1 | 24.7 | 24.7 | 24.7 |
| claude PreToolUse, concurrent | 20 | 4.2 | 5.7 | 5.8 | 5.8 |
| codex PreToolUse, concurrent | 20 | 5.1 | 6.2 | 8.2 | 8.2 |

Observe day:

| Agent and event | n | p50 | p95 | p99 | max |
| --- | --- | --- | --- | --- | --- |
| claude PreToolUse | 74 | 3.9 | 5.6 | 8.3 | 8.3 |
| claude PostToolUse | 74 | 3.9 | 6.4 | 8.3 | 8.3 |
| claude Stop | 9 | 15.6 | 66.5 | 66.5 | 66.5 |
| codex PreToolUse | 74 | 3.9 | 4.9 | 5.8 | 5.8 |
| codex PostToolUse | 74 | 3.8 | 4.6 | 5.0 | 5.0 |
| codex Stop | 9 | 10.6 | 11.3 | 11.3 | 11.3 |

Stop is slower because it runs the Stop gate's shell command, fingerprints the Git tree while a plan is active, and may start a background digest worker.
Every measurement stays far below the shortest documented hook budget, the 1 second Codex SessionEnd default.
