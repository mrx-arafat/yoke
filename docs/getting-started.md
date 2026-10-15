# Getting started

[Documentation index](README.md)

## Prerequisites

Use a recent stable Rust toolchain with Cargo and a local checkout of this repository.
Agent integration requires Claude Code or Codex installed separately.
Independent review and memory digests additionally require an authenticated agent CLI.
SQLite is bundled through the Rust dependency, so a separate database server is unnecessary.

## 1. Build and install the executable

To install or upgrade without a checkout, run the install script:

```sh
curl -fsSL https://raw.githubusercontent.com/mrx-arafat/yoke/main/install.sh | bash
```

It supports macOS and Linux on x86_64 and aarch64, and it never uses `sudo`.
It downloads `yoke-<target>` and `yoke-<target>.sha256` from the latest GitHub release, the same assets `yoke update` uses, and refuses to install unless the SHA-256 checksum matches.
When the release has no executable for your platform, including while no release is published, it builds from source with `cargo install --locked --git https://github.com/mrx-arafat/yoke yoke-cli`.
If Cargo is missing, it asks before installing Rust with rustup; without a terminal it stops with the exact command unless `YOKE_INSTALL_RUST=1` is set.
It replaces the executable atomically, runs `yoke install` for the agents it finds, creates or migrates the state database with `yoke log` once the hooks run the new executable, and reports `yoke health`.
With `YOKE_AGENTS=none`, or when it finds no agent, it leaves the database alone, so hooks that still run an older executable keep working.
If your hooks run a Yoke executable from another location, it runs `yoke uninstall` with the new executable and then installs again.
These environment variables change what it does:

| Variable | Effect |
| --- | --- |
| `YOKE_INSTALL_DIR` | Absolute install directory, default `~/.local/bin`. |
| `YOKE_VERSION` | Release tag to download, or to build with `--tag`, instead of the latest. |
| `YOKE_DOWNLOAD_URL` | Base URL of a mirror that serves the release assets. |
| `YOKE_FROM_SOURCE=1` | Build from source even when a release exists. |
| `YOKE_INSTALL_RUST=1` | Install Rust without asking when Cargo is missing. |
| `YOKE_AGENTS` | Agents to connect, such as `claude` or `claude,codex`, or `none`; by default each agent found on `PATH` or by its configuration directory. |

To build from a local checkout instead, from the repository root:

```sh
cargo build --release
./target/release/yoke --version
cargo install --path crates/yoke-cli --locked
yoke --help
```

To build straight from GitHub without the script: `cargo install --locked --git https://github.com/mrx-arafat/yoke yoke-cli`.
Cargo installs the executable into its configured binary directory, normally `~/.cargo/bin`.
Ensure that directory is on your PATH.
The remaining commands run from the project you want Yoke to govern, which need not be Yoke's own repository.

## 2. Create and approve a project policy

```sh
yoke init
```

This creates `yoke.toml` without overwriting an existing file.
The starter uses observe mode and contains a sample `no-force-push` gate matching the `Bash` tool and the literal text `push --force`.
It is an example matcher, not comprehensive protection against every force-push spelling or tool name.
Inspect and edit the file for your agent's actual payloads before enabling enforcement.

From your own interactive terminal:

```sh
yoke trust
yoke config
yoke gates
```

Review the displayed snapshot and type the requested confirmation phrase.
Trust cannot be approved noninteractively or from a detected agent tool process.
Subsequent edits remain inactive until you trust them again.

## 3. Exercise the starter gate safely

```sh
printf '%s\n' '{"tool_name":"Bash","tool_input":{"command":"git push --force"}}' |
  yoke run-hook --agent claude pre_tool_use
yoke log --outcome would_deny --limit 5
```

This sends a simulated payload; it does not execute Git.
Observe mode produces no hook output, and the decision log should contain `would_deny` when the trusted starter gate matches.
If it does not, inspect [troubleshooting](troubleshooting.md).

## 4. Connect an agent

```sh
yoke install --agent claude,codex --dry-run
yoke install --agent claude,codex
```

Select only `claude` or `codex` if you use one agent.
Installation preserves unrelated hooks and creates backups and ownership receipts.
It is idempotent, and every hook it installs carries a label such as `Yoke: PreToolUse` in both agents.
After updating Yoke, rerun `yoke install --agent claude,codex` to label existing Yoke-owned hooks without adding duplicates.
Hooks you registered manually or changed yourself remain untouched.
For Codex, review new hooks using `/hooks` in Codex itself.
If you run Claude Code with `CLAUDE_CONFIG_DIR` or Codex with `CODEX_HOME`, keep the same variable set when you run `yoke install`, so the hooks land in the files the agent reads.
Restart or reload your agent's hook configuration as appropriate, perform a normal tool action, and inspect `yoke log` for the resulting decision.
Run `yoke health` to confirm that each agent reports installed, enabled hooks and a recorded event; it exits 1 when a check fails.

For Claude's optional statusline:

```sh
yoke install --agent claude --statusline
```

This wraps an existing statusline command while preserving its input and settings.
Uninstall restores the prior statusline when Yoke still owns it and preserves your subsequent changes.

### Register hooks manually

If you register hooks yourself instead of using `yoke install`, include the agent name in each invocation.
For example, Claude Code settings can include:

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "*", "hooks": [{ "type": "command", "command": "yoke run-hook --agent claude PreToolUse", "statusMessage": "Yoke: PreToolUse" }] }
    ],
    "Stop": [
      { "hooks": [{ "type": "command", "command": "yoke run-hook --agent claude Stop", "statusMessage": "Yoke: Stop" }] }
    ]
  }
}
```

## 5. Enable enforcement when ready

Set the top-level `mode` in `yoke.toml` to `"enforce"`, then run `yoke trust` again from your own terminal.
Repeat the simulated payload above to inspect the agent-specific denial response.
Only `pre_tool_use`, `user_prompt_submit`, and `stop` can block; other events report failures without vetoing the action.

To remove managed hooks, inspect and apply:

```sh
yoke uninstall --agent claude,codex --dry-run
yoke uninstall --agent claude,codex
```

Uninstall removes Yoke-owned integration entries rather than erasing project policy or memory.
Continue with the [configuration reference](configuration.md) to define your own gates.
