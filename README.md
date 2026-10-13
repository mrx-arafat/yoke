# Yoke

Steer your coding agent with gates you define.

Yoke is a small, fast Rust binary that runs as a hook handler for coding agents such as Claude Code.
On each agent event it runs the checks you configured, and on blocking events it can veto the action with a reason the agent sees.
It is free and open source under MIT OR Apache-2.0.

## Status

Version 0.1.0 is a working skeleton: config loading, the gate engine, and the hook CLI.
See the roadmap below for what comes next.

## Build

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
echo '{"tool_name":"Write","tool_input":{"file_path":".env"}}' | yoke run-hook pre_tool_use
```

## Wire it into an agent

Yoke reads the agent's hook payload as JSON on stdin and answers in the documented hook-output shape.
For Claude Code, register it in `settings.json` (check the agent's current hooks documentation for the exact schema):

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

1. Persistent project memory with a local SQLite store.
2. Workflow engine for multi-step plan, build and verify loops.
3. Local web console for reviewing plans, diffs and gate history.
4. Codex adapter alongside Claude Code.
5. Fast statusline command.

## Clean-room policy

Yoke is an independent implementation built only from public documentation and public, idea-level concepts.
See [COMPLIANCE.md](COMPLIANCE.md) for the rules every contribution follows.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
