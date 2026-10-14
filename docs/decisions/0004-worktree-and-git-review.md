# 0004 Worktree lifecycle and Git review surfaces: read-only diff only

Status: accepted on 2026-10-14 for issue #21.

## Context

Yoke is a local trusted gate harness: hooks decide tool calls, plans freeze approved checks, the stop guard holds completion, and an independent reviewer judges the change.
Its Git use is read-only evidence collection.
`plan start` records the current `HEAD` as `base_ref` (`crates/yoke-core/src/plan.rs:302`).
Review input is `git diff <base_ref>` of tracked files plus the content of untracked, nonignored files, capped at 1 MiB (`crates/yoke-core/src/review/input.rs:16`, `crates/yoke-core/src/review/input.rs:60`).
Both `yoke plan check` (`crates/yoke-core/src/plan/checks.rs:39`) and `yoke review` (`crates/yoke-cli/src/workflow.rs:406`) pass the plan's `base_ref` to that same collector.
Before this change nobody could see that exact input without running a reviewer.

Both supported agents already manage worktrees natively.
Claude Code starts sessions in worktrees with `--worktree`, isolates subagents with `isolation: worktree`, exposes `WorktreeCreate` and `WorktreeRemove` hooks, and on exit checks a worktree for changed, untracked or committed work before offering removal (https://code.claude.com/docs/en/worktrees.md, https://code.claude.com/docs/en/sub-agents.md, https://code.claude.com/docs/en/hooks.md).
Codex creates managed worktrees in `$CODEX_HOME/worktrees`, moves chats between Local and Worktree with Handoff, keeps the 15 most recent managed worktrees by default, and offers `codex exec --worktree` (https://developers.openai.com/codex/app/worktrees, https://developers.openai.com/codex/cli/reference).

## Decision

Worktree lifecycle management and a diff, stage and commit workbench do not belong in Yoke now.
Both agents own worktree state natively, and a second owner of the same branches and directories would race with them.
Git writes are the operations the bundled `git-safety` preset exists to stop (`crates/yoke-core/src/preset.rs:96`); a gate harness that performs them would itself need a stronger gate than the agents it constrains.
Yoke's distinct value is trustworthy evidence, so the part that fits is making review evidence inspectable.

### Implemented: read-only review input

`yoke plan diff <slug> [--json]` prints the exact change text the independent review would receive for that plan.
It calls the reviewer's own collector through `yoke_core::review::input`, which delegates to `review::input::collect` with the plan's `base_ref`, falling back to `HEAD` for plans that have not started, exactly as review does.
Text output lists the plan, state, base, byte count and untracked file count, then every line JSON-quoted so terminal control sequences in reviewed files stay inert.
`--json` returns the same fields with the unquoted `diff` string.
It runs no reviewer, writes no Git state and records no evidence.
Errors are the reviewer's own, such as binary tracked changes or input above 1 MiB, so a failing `plan diff` predicts a failing review.

### Interactions to fix before any worktree feature

Trust snapshots and plans are keyed by the canonical project root (`crates/yoke-core/src/trust.rs:194`), so each linked worktree is a separate Yoke project that needs its own `yoke trust`.
A plan stores the project path it was created in, so if an agent moves a session into a worktree mid-plan, `plan check` still reads the original checkout and `yoke review` run from the worktree cannot find the plan.
Both belong to session project identity work (issue #8) and should be resolved before Yoke automates anything around worktrees.

## Specification if worktree automation is approved later (not implemented)

Authorization: every Git write (create, sync, commit, remove) requires a human terminal outside an agent process and a typed confirmation naming the operation and path, like `yoke plan approve`; no hook may trigger a write.
Creation: Yoke creates a worktree only from a clean, named commit, under a Yoke-owned directory, on a new branch whose name it records in a receipt before running `git worktree add`; it refuses if the branch or path exists.
Handoff: a plan moves to a worktree only through an explicit `plan reassign`-style command that records the new project path, keeps `base_ref`, and invalidates prior check evidence.
Sync: Yoke never rebases, merges or resets; it shows `git diff` and `git log` between the worktree and its base and leaves integration to the human or the agent's native Handoff.
Cleanup: removal is refused while the worktree has changed or untracked files, unpushed commits, or a live plan, mirroring the checks Claude Code documents; otherwise Yoke removes only the worktree and branch its receipt names.
Recovery: an interrupted create or remove is detected from the receipt and either completed or reported for the human, never guessed.
Preservation: Yoke never stashes, checks out, or discards user changes in any checkout, and refuses to act while another Yoke process holds the plan lock.
