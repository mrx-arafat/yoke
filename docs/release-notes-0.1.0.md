# Yoke 0.1.0 Release Notes (Draft)

This document is a draft for review and is not the published release notes.

## Summary

Yoke sits in the hook path of Claude Code and Codex.
On every hook event it runs the checks a human approved, records what it decided, and in enforce mode blocks the action with a reason the agent sees.
A plan is marked done only after Yoke itself has run the plan's checks on the final working tree.
It is one binary and one SQLite file on your machine, with no daemon and no telemetry.

## Install

One-line installer (macOS and Linux, x86_64 and aarch64):

```sh
curl -fsSL https://raw.githubusercontent.com/mrx-arafat/yoke/main/install.sh | bash
```

Alternative, from a local checkout:

```sh
cargo install --path crates/yoke-cli --locked
```

## Highlights

- Claude Code and Codex hook handling with observe/enforce modes, shell checks, denials, command rewrites, bounded execution, and redacted SQLite decision logs.
- Human-approved global and project configuration snapshots with partial-layer overrides, exact-byte hashes, origin reporting, and stale-file diagnostics.
- Reversible agent hook installation with dry runs, backups, ownership receipts, and optional Claude statusline integration.
- Git-backed plan files with frozen human approval, database-owned states, session binding, explicit human resume, bounded stop guards, and actual check execution before completion.
- Independent review and persisted check evidence bound to working-tree fingerprints, including untracked nonignored files and concurrent-change detection.
- Local facts and remembered notes, full-text recall, optional session digests, explicitly trusted shared knowledge, and portable memory import/export with project mapping and duplicate protection.
- A token-authenticated loopback console for sessions, plans, decisions, memory, trust configuration, and usage, with light/dark themes, keyboard navigation, approval confirmation, and memory deletion Undo.
- Statusline mode, plan state, alerts, Claude quota (5-hour and weekly percent used and reset time) and optional existing-command wrapping.
- `yoke usage` for each agent's quota, usage estimates that leave unsupported costs unknown, and the newest sessions.
- Readable git-safety, web-redirect, token-saver, and UI presets that require explicit configuration trust before activation.
- Checksum-verified self-update with atomic binary replacement.

## Upgrading from a source build

If you already run a Yoke binary built from source, read the full [upgrade notes](upgrade-notes.md) before replacing it.
The key steps and warnings:

1. Replace every Yoke executable that runs hooks at the same time, including any `target/release/yoke` that installed hooks point at.
   The first writable open by the new binary migrates the state database from schema 6 to 12, and an older binary then refuses it on every hook event.
2. After replacing, rerun `yoke install --agent claude,codex` with the same `CLAUDE_CONFIG_DIR` and `CODEX_HOME` you start the agents with, then run `yoke health`.
3. Codex runs new hooks only after you approve them with `/hooks` inside Codex.
4. Do not trust a configuration that uses the new `[review.models]` or `[memory.digest_models]` keys until every hook binary is replaced.
5. Keep a copy of `~/.yoke/state.db` with its `-wal` and `-shm` files if you may need to go back.
   There is no downgrade command.

## Supported platforms and requirements

- macOS and Linux on x86_64 and aarch64.
- You need Claude Code or Codex.
- While no release is published, installing builds from source, so you also need a stable Rust toolchain.
- On Alpine, use `| sh` instead of `| bash`, because Alpine ships without bash.
- Release executables need glibc, so on Alpine and other musl systems the installer always builds from source.

## Known limitations

- Yoke is not a sandbox.
  Its self-protection stops an agent from casually approving its own plan, starting the console or editing Yoke state through ordinary tools, but an agent with a shell running as your OS user can deliberately work around it.
- Residual guard bypasses: a determined agent can hide a command behind a shell alias, a variable, a copied or renamed `yoke` binary, an encoded payload, or string concatenation inside an interpreter, remove the agent markers and fake a terminal, use an unused console link, open the database with its own tools, or replace the `yoke` binary or the hook settings.
- Trusting a configuration authorizes its shell commands to run with your permissions, and observe mode still runs them.
- An actor recorded as `human` means the action passed the human-terminal check, not that a person was proven to act.
- Evidence comparisons have a size limit: tracked or untracked files up to 64 MiB are content-hashed, but larger ones are compared only by size and modification time to the nanosecond.
- Pending acceptance items:
  - The full-working-day observe-mode trial and a week of real work with Yoke as the only enforce-mode harness for both agents remain pending.
  - Cross-platform runtime acceptance across all supported release targets remains pending.
  - Lighthouse performance acceptance and a recorded before/after performance baseline remain pending.
  - Published release distribution and end-to-end self-update acceptance remain unverified.

## Checksums

The release workflow attaches a `.sha256` file for every asset.
To verify an asset after downloading it, run:

```sh
shasum -a 256 -c <asset>.sha256
```

No digests are listed in this draft; the published release notes will carry the digests from the release workflow run.
