# Yoke

Steer your coding agent with gates you define.

Yoke is a small, fast Rust binary that sits in the hook path of coding agents such as Claude Code and Codex.
On every agent event it runs the checks you configured, records what it decided, and on blocking events it can veto the action with a reason the agent sees.
It is free and open source under MIT OR Apache-2.0.

## Why Yoke

Coding agents follow instructions most of the time.
Yoke is for the rest of the time.

- **Code enforces, prompts inform.** Rules that must hold are checked by a binary, not left to the model's goodwill.
- **Proof, not promises.** A plan is only done when Yoke itself has run its checks on the final code.
- **Nothing silent.** Every allow, deny and error is logged; when Yoke breaks, it says so and fails closed.
- **Safe to try.** New installs start in observe mode: Yoke logs what it would block and blocks nothing.
- **Local and light.** One binary, one SQLite file, no daemon, no telemetry.

## Status

Version 0.1.0 is a working skeleton: config loading, the gate engine and the hook CLI.
Everything marked *planned* below is designed in [the design spec](docs/specs/2026-10-13-yoke-design.md) and not built yet.

> **Security note for 0.1:** Yoke runs the `run` commands of any `yoke.toml` it finds at or above the working directory.
> Only use it in repositories you trust.
> Trusted config snapshots, which close this gap, ship in 0.2.

## Features

| Feature | What it does | Status |
|---------|--------------|--------|
| Gates | Shell checks on agent events; a failing gate blocks the action on blocking events | Available |
| Layered config | Global `~/.yoke/config.toml` merged with the nearest project `yoke.toml` | Available |
| Decision log | Every hook decision stored with secrets masked, searchable with `yoke log` | Planned, 0.2 |
| Observe and enforce modes | Shadow-run beside another harness before letting Yoke block anything | Planned, 0.2 |
| Trusted config | Config runs only from snapshots a human approved with `yoke trust` | Planned, 0.2 |
| One-command install | `yoke install --agent claude,codex`, fully reversible with `yoke uninstall` | Planned, 0.2 |
| Self-update | `yoke update` with checksum verification and an atomic swap | Planned, 0.2 |
| Plans | Plan files in git, human approval, states the agent cannot fake | Planned, 0.3 |
| Evidence checks | `done` only after Yoke runs the plan's checks on the final working tree | Planned, 0.3 |
| Stop guard | Keeps the agent working while a plan is open, with a cap so it cannot loop forever | Planned, 0.3 |
| Independent review | A second model reviews the diff and blocking findings fail the plan | Planned, 0.3 |
| Memory | Facts, remembered notes, an optional session digest and full-text recall | Planned, 0.4 |
| Shared knowledge | Promote a memory into `.yoke/knowledge.md` so the whole team gets it | Planned, 0.4 |
| Console | A local web console for plans, decisions, memory and usage | Planned, 0.5 |
| Statusline | Mode, plan state and alerts in the agent's status bar, under 15 ms | Planned, 0.5 |
| Presets | Ready-made gate packs such as `git-safety`, plus command rewrites | Planned, 0.6 |

## Build

Requires a recent stable Rust toolchain.

```bash
cargo build --release
# binary: target/release/yoke
```

## Quick start

```bash
yoke init     # writes a starter yoke.toml
yoke gates    # lists the gates that apply in this directory
```

Try a gate by hand:

```bash
echo '{"tool_name":"Bash","tool_input":{"command":"git push --force"}}' | yoke run-hook pre_tool_use
```

The starter config blocks force pushes, so this prints a deny decision.

## Wire it into an agent

Yoke reads the agent's hook payload as JSON on stdin and answers in the agent's documented hook-output format.
Until `yoke install` ships in 0.2, register it by hand.
For Claude Code, add this to `settings.json` (check the agent's current hooks documentation for the exact schema):

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "*", "hooks": [{ "type": "command", "command": "yoke run-hook PreToolUse" }] }
    ],
    "Stop": [
      { "hooks": [{ "type": "command", "command": "yoke run-hook Stop" }] }
    ]
  }
}
```

## Configuration

Yoke merges `~/.yoke/config.toml` (global) with the nearest `yoke.toml` at or above the working directory (project).
A project gate replaces a global gate with the same name.

| Key | Required | Meaning |
|-----|----------|---------|
| `name` | yes | Unique gate name |
| `on` | yes | Event: `pre_tool_use`, `post_tool_use`, `user_prompt_submit`, `stop`, `session_start`, `session_end`, `notification` |
| `run` | yes | Shell command; a non-zero exit fails the gate |
| `match_tool` | no | Only run for this tool name |
| `match_contains` | no | Only run when the payload contains this text |
| `message` | no | Reason shown to the agent on failure (defaults to the command's stderr) |
| `shell` | no | Shell used as `<shell> -c <run>` (default `sh`) |

Gate commands receive `YOKE_EVENT`, `YOKE_GATE`, `YOKE_TOOL_NAME` and `YOKE_TOOL_INPUT` in their environment.

## How decisions work

On `pre_tool_use`, `user_prompt_submit` and `stop`, any failing gate blocks the action.
On every other event, failures are reported but the agent continues.
When nothing fails, Yoke prints nothing and the agent proceeds normally.

## Roadmap

The full design, with the reasoning behind each decision, is in [docs/specs/2026-10-13-yoke-design.md](docs/specs/2026-10-13-yoke-design.md).

1. **0.2 Core:** decision log, observe and enforce modes, trusted config snapshots, Claude Code and Codex install, self-update.
2. **0.3 Workflow:** plans with human approval, checks Yoke runs itself, stop guard, independent review.
3. **0.4 Memory:** facts, remembered notes, optional session digest, full-text recall, knowledge shared through git.
4. **0.5 Surfaces:** a world-class local web console and a fast statusline.
5. **0.6 Presets:** ready-made gate packs, command rewrites, UI checks.

Version 1.0 is the point where Yoke runs in enforce mode for both agents as the only harness, for a week of real work.

## Clean-room policy

Yoke is an independent implementation built only from public documentation and public, idea-level concepts.
See [COMPLIANCE.md](COMPLIANCE.md) for the rules every contribution follows.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
